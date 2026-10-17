//! [`MosaicSampling`]: the photosites of a mosaic a noise statistic samples.

use std::f32::consts::SQRT_2;

use arrayvec::ArrayVec;
use imaginarium::Buffer2;

use crate::image::background_split::BackgroundSplit;
use crate::image::cfa::cfa_type::CfaType;
use crate::image::flat_gain::FlatGain;
use crate::math::size2us::Size2us;
use crate::math::statistics::median_mad::MedianMad;
use crate::math::vec2us::Vec2us;
use crate::noise::background_split;
use crate::noise::binned_samples::BinnedSamples;

/// The photosites of a `size` mosaic of `cfa_type` a statistic samples: those of the rows
/// [`CfaType::sampled_rows`] takes for about `per_color` samples a color, less the pixels
/// `excluded` names.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MosaicSampling<'a, E> {
    pub(crate) size: Size2us,
    pub(crate) cfa_type: &'a CfaType,
    pub(crate) excluded: E,
    pub(crate) per_color: usize,
}

/// A photosite and the nearest same-color photosite to its right.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Pair {
    x: usize,
    neighbour: usize,
    y: usize,
    pub(crate) color: usize,
}

impl Pair {
    /// The photosite's sample less its neighbour's, in the `mosaic` sampled.
    pub(crate) fn difference(self, mosaic: &Buffer2<f32>) -> f32 {
        let row = mosaic.row(self.y);
        row[self.x] - row[self.neighbour]
    }

    /// The flat `gain` of the pair: the mean of its two photosites' gains.
    pub(crate) fn gain(self, gain: &FlatGain) -> f32 {
        let y = self.y as f32;
        f32::midpoint(
            gain.at(self.color, self.x as f32, y),
            gain.at(self.color, self.neighbour as f32, y),
        )
    }
}

impl<E: Fn(usize) -> bool> MosaicSampling<'_, E> {
    /// The index and color of every sampled photosite.
    pub(crate) fn for_each_photosite(&self, mut visit: impl FnMut(usize, usize)) {
        let Size2us { width, .. } = self.size;
        for y in self.cfa_type.sampled_rows(self.size, self.per_color) {
            for x in 0..width {
                let index = y * width + x;
                if !(self.excluded)(index) {
                    visit(index, self.cfa_type.slot_at(Vec2us::new(x, y)));
                }
            }
        }
    }

    /// How many photosites of each color [`Self::for_each_photosite`] visits.
    pub(crate) fn photosite_counts(&self) -> [usize; 3] {
        let mut counts = [0; 3];
        self.for_each_photosite(|_, color| counts[color] += 1);
        counts
    }

    /// Every pair of a sampled photosite and the nearest same-color photosite to its right in its
    /// row; a pair touching an excluded pixel is skipped.
    pub(crate) fn for_each_pair(&self, mut visit: impl FnMut(Pair)) {
        let Size2us { width, .. } = self.size;
        for y in self.cfa_type.sampled_rows(self.size, self.per_color) {
            for x in 0..width {
                let color = self.cfa_type.slot_at(Vec2us::new(x, y));
                // Every color recurs within a period on every row of its own.
                let Some(neighbour) = (x + 1..(x + 1 + CfaType::MAX_PERIOD).min(width))
                    .find(|&nx| self.cfa_type.slot_at(Vec2us::new(nx, y)) == color)
                else {
                    continue;
                };
                if (self.excluded)(y * width + x) || (self.excluded)(y * width + neighbour) {
                    continue;
                }
                visit(Pair {
                    x,
                    neighbour,
                    y,
                    color,
                });
            }
        }
    }

    /// How many pairs of each color [`Self::for_each_pair`] visits.
    pub(crate) fn pair_counts(&self) -> ArrayVec<usize, 3> {
        let mut counts: ArrayVec<usize, 3> = (0..self.cfa_type.num_colors()).map(|_| 0).collect();
        self.for_each_pair(|pair| counts[pair.color] += 1);
        counts
    }

    /// The noise standard deviation of each color of `mosaic` over the pairs sampled, indexed by
    /// color, from differences of same-color neighbours. A color with fewer than two pairs reads
    /// `0`.
    ///
    /// A smoothing kernel on a mosaic mixes colors, so the multiresolution estimator does not
    /// apply. The difference of a photosite and the nearest same-color photosite to its right
    /// cancels any signal that changes slowly over that distance, and leaves the two noises: `√2·σ`
    /// for white noise. MAD makes the stars and the hits a minority that does not count. Every row
    /// of a Bayer and of an X-Trans pattern holds all three colors, so the neighbour is at most six
    /// photosites away.
    pub(crate) fn noise(&self, mosaic: &Buffer2<f32>) -> ArrayVec<f32, 3> {
        debug_assert_eq!(Size2us::from(mosaic), self.size, "the mosaic sampled");
        let mut differences: ArrayVec<Vec<f32>, 3> = self
            .pair_counts()
            .into_iter()
            .map(Vec::with_capacity)
            .collect();
        self.for_each_pair(|pair| {
            differences[pair.color].push(pair.difference(mosaic));
        });
        differences.iter_mut().map(|color| sigma(color)).collect()
    }

    /// Each color's background under the flat `gain` divided `mosaic` by: the noise of
    /// [`Self::noise`] measured over each of the color's gain bins on its own, a pair at the mean
    /// of its two photosites' gain, and fitted by [`background_split::fit`]; unflattened, of
    /// [`Self::noise`], for a mosaic no flat divided.
    pub(crate) fn backgrounds(
        &self,
        mosaic: &Buffer2<f32>,
        gain: Option<&FlatGain>,
    ) -> ArrayVec<BackgroundSplit, 3> {
        let Some(gain) = gain else {
            return self
                .noise(mosaic)
                .into_iter()
                .map(BackgroundSplit::of_sigma)
                .collect();
        };
        debug_assert_eq!(Size2us::from(mosaic), self.size, "the mosaic sampled");
        let mut binned: ArrayVec<BinnedSamples<Vec<f32>>, 3> = self
            .pair_counts()
            .into_iter()
            .map(BinnedSamples::with_capacity)
            .collect();
        self.for_each_pair(|pair| {
            binned[pair.color].push(
                gain.bins(pair.color),
                pair.gain(gain),
                pair.difference(mosaic),
            );
        });
        binned
            .into_iter()
            .map(|color| {
                background_split::fit(
                    &color.measure(|differences| f64::from(sigma(differences)).powi(2)),
                )
            })
            .collect()
    }

    /// The most bytes an estimate over the mosaic holds: every pair of every color, as
    /// [`Self::backgrounds`] holds them, each with its gain bin.
    pub(crate) fn noise_heap_bytes(&self) -> usize {
        self.pair_counts()
            .into_iter()
            .map(BinnedSamples::<Vec<f32>>::heap_bytes)
            .sum()
    }
}

/// The white noise σ of `differences`, each the difference of two independent samples, from their
/// MAD; `0` for fewer than two. Reorders `differences`.
fn sigma(differences: &mut [f32]) -> f32 {
    if differences.len() < 2 {
        return 0.0;
    }
    MedianMad::of_mut(differences).sigma() / SQRT_2
}

#[cfg(test)]
mod tests {
    use imaginarium::Buffer2;

    use crate::image::cfa::cfa_pattern::CfaPattern;
    use crate::image::cfa::cfa_type::CfaType;
    use crate::internals::test_rng::TestRng;
    use crate::math::size2us::Size2us;
    use crate::math::statistics::subsample::MAX_STATISTIC_SAMPLES;
    use crate::math::vec2us::Vec2us;
    use crate::noise::mosaic_sampling::MosaicSampling;

    /// Each color of a Bayer mosaic reads its own noise, through a gradient the differences
    /// cancel. σ = 0.01 red, 0.02 green, 0.03 blue on a 512² mosaic: every row holds 255 pairs of
    /// each of its two colors, so red and blue keep 256 · 255 = 65 280 pairs each, green 130 560.
    /// At 43 690 samples a color the rows step by ⌊262 144 / 131 070⌋ = 2 periods, half the rows,
    /// where a step of 2 single rows would read no blue row at all: 32 640 pairs of red and blue.
    /// MAD's standard error is 1.166·σ/√n, so 3 of them is at most 1.94% of σ; 2% covers both.
    #[test]
    fn each_bayer_color_reads_its_own_noise() {
        let size = Size2us::new(512, 512);
        let cfa_type = CfaType::Bayer(CfaPattern::Rggb);
        let sigmas = [0.01f32, 0.02, 0.03];
        let mut rng = TestRng::new(9);
        let mosaic = Buffer2::from_fn(512, 512, |x, y| {
            let color = cfa_type.slot_at(Vec2us::new(x, y));
            0.2 + 0.3 * (x as f32 / 512.0) + sigmas[color] * rng.next_gaussian_f32()
        });
        for per_color in [MAX_STATISTIC_SAMPLES, 43_690] {
            let sampling = MosaicSampling {
                size,
                cfa_type: &cfa_type,
                excluded: |_| false,
                per_color,
            };
            let estimates = sampling.noise(&mosaic);
            for (color, (&estimate, &sigma)) in estimates.iter().zip(&sigmas).enumerate() {
                assert!(
                    (estimate - sigma).abs() < 0.02 * sigma,
                    "{per_color} a color, color {color}: {estimate}"
                );
            }
        }
    }
}
