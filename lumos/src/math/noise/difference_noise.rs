//! [`DifferenceNoise`]: the white noise of each colour of a CFA mosaic.

use std::f32::consts::SQRT_2;

use arrayvec::ArrayVec;

use crate::image::cfa::cfa_type::CfaType;
use crate::image::flat_gain::FlatGain;
use crate::math::noise::background_split::BackgroundSplit;
use crate::math::noise::binned_samples::BinnedSamples;
use crate::math::noise::mosaic_sampling::MosaicSampling;
use crate::math::size2us::Size2us;
use crate::math::statistics;
use crate::math::statistics::subsample::MAX_STATISTIC_SAMPLES;
use imaginarium::Buffer2;

/// The noise standard deviation of each colour of a CFA mosaic, from differences of same-colour
/// neighbours.
///
/// A smoothing kernel on a mosaic mixes colours, so the multiresolution estimator does not apply.
/// The difference of a photosite and the nearest same-colour photosite to its right cancels any
/// signal that changes slowly over that distance, and leaves the two noises: `√2·σ` for white
/// noise. MAD makes the stars and the hits a minority that does not count. Every row of a Bayer and
/// of an X-Trans pattern holds all three colours, so the neighbour is at most six photosites away.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DifferenceNoise;

impl DifferenceNoise {
    /// The noise standard deviation of each colour of `mosaic` over the pairs `sampling` takes,
    /// indexed by colour. A colour with fewer than two pairs reads `0`.
    pub(crate) fn estimate(
        mosaic: &Buffer2<f32>,
        sampling: &MosaicSampling<'_, impl Fn(usize) -> bool>,
    ) -> ArrayVec<f32, 3> {
        debug_assert_eq!(Size2us::from(mosaic), sampling.size, "the mosaic sampled");
        let mut differences: ArrayVec<Vec<f32>, 3> = sampling
            .pair_counts()
            .into_iter()
            .map(Vec::with_capacity)
            .collect();
        sampling.for_each_pair(|pair| {
            differences[pair.colour].push(pair.difference(mosaic));
        });
        differences.iter_mut().map(|colour| sigma(colour)).collect()
    }

    /// Each colour's background under the flat `gain` divided `mosaic` by: the noise measured over
    /// each of the colour's gain bins on its own, a pair at the mean of its two photosites' gain,
    /// and fitted by [`BackgroundSplit::fit`].
    pub(crate) fn estimate_split(
        mosaic: &Buffer2<f32>,
        sampling: &MosaicSampling<'_, impl Fn(usize) -> bool>,
        gain: &FlatGain,
    ) -> ArrayVec<BackgroundSplit, 3> {
        debug_assert_eq!(Size2us::from(mosaic), sampling.size, "the mosaic sampled");
        let mut binned: ArrayVec<BinnedSamples<Vec<f32>>, 3> = sampling
            .pair_counts()
            .into_iter()
            .map(BinnedSamples::with_capacity)
            .collect();
        sampling.for_each_pair(|pair| {
            binned[pair.colour].push(
                gain.bins(pair.colour),
                pair.gain(gain),
                pair.difference(mosaic),
            );
        });
        binned
            .into_iter()
            .map(|colour| {
                BackgroundSplit::fit(
                    &colour.measure(|differences| f64::from(sigma(differences)).powi(2)),
                )
            })
            .collect()
    }

    /// The most bytes an estimate over a mosaic of `size` and `cfa_type` holds: every pair of
    /// every colour, as [`Self::estimate_split`] holds them, each with its gain bin.
    pub(crate) fn heap_bytes(size: Size2us, cfa_type: &CfaType) -> usize {
        MosaicSampling {
            size,
            cfa_type,
            excluded: |_| false,
            per_colour: MAX_STATISTIC_SAMPLES,
        }
        .pair_counts()
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
    let centre = statistics::median_mut(differences);
    for value in differences.iter_mut() {
        *value = (*value - centre).abs();
    }
    statistics::mad_to_sigma(statistics::median_mut(differences)) / SQRT_2
}

#[cfg(test)]
mod tests {
    use imaginarium::Buffer2;

    use crate::image::cfa::cfa_pattern::CfaPattern;
    use crate::image::cfa::cfa_type::CfaType;
    use crate::internals::test_rng::TestRng;
    use crate::math::noise::difference_noise::DifferenceNoise;
    use crate::math::noise::mosaic_sampling::MosaicSampling;
    use crate::math::size2us::Size2us;
    use crate::math::statistics::subsample::MAX_STATISTIC_SAMPLES;
    use crate::math::vec2us::Vec2us;

    /// Each colour of a Bayer mosaic reads its own noise, through a gradient the differences
    /// cancel. σ = 0.01 red, 0.02 green, 0.03 blue on a 512² mosaic: every row holds 255 pairs of
    /// each of its two colours, so red and blue keep 256 · 255 = 65 280 pairs each, green 130 560.
    /// At 43 690 samples a colour the rows step by ⌊262 144 / 131 070⌋ = 2 periods, half the rows,
    /// where a step of 2 single rows would read no blue row at all: 32 640 pairs of red and blue.
    /// MAD's standard error is 1.166·σ/√n, so 3 of them is at most 1.94% of σ; 2% covers both.
    #[test]
    fn each_bayer_colour_reads_its_own_noise() {
        let size = Size2us::new(512, 512);
        let cfa_type = CfaType::Bayer(CfaPattern::Rggb);
        let sigmas = [0.01f32, 0.02, 0.03];
        let mut rng = TestRng::new(9);
        let mosaic = Buffer2::from_fn(512, 512, |x, y| {
            let colour = cfa_type.color_at(Vec2us::new(x, y));
            0.2 + 0.3 * (x as f32 / 512.0) + sigmas[colour] * rng.next_gaussian_f32()
        });
        for per_colour in [MAX_STATISTIC_SAMPLES, 43_690] {
            let sampling = MosaicSampling {
                size,
                cfa_type: &cfa_type,
                excluded: |_| false,
                per_colour,
            };
            let estimates = DifferenceNoise::estimate(&mosaic, &sampling);
            for (colour, (&estimate, &sigma)) in estimates.iter().zip(&sigmas).enumerate() {
                assert!(
                    (estimate - sigma).abs() < 0.02 * sigma,
                    "{per_colour} a colour, colour {colour}: {estimate}"
                );
            }
        }
    }
}
