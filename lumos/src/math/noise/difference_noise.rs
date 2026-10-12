//! [`DifferenceNoise`]: the white noise of each colour of a CFA mosaic.

use std::f32::consts::SQRT_2;

use arrayvec::ArrayVec;

use crate::image::cfa::cfa_type::CfaType;
use crate::image::flat_gain::FlatGain;
use crate::math::noise::background_split::BackgroundSplit;
use crate::math::noise::binned_samples::BinnedSamples;
use crate::math::size2us::Size2us;
use crate::math::statistics::subsample::MAX_STATISTIC_SAMPLES;
use crate::math::statistics::{mad_to_sigma, median_mut};
use crate::math::vec2us::Vec2us;

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

/// The farthest a same-colour neighbour can be in a row: the X-Trans period.
const MAX_REACH: usize = 6;

impl DifferenceNoise {
    /// The noise standard deviation of each colour of `cfa_type` in `mosaic`, a row-major `size`
    /// image, indexed by colour; pairs touching a pixel `excluded` names are skipped. A colour with
    /// fewer than two pairs reads `0`.
    pub(crate) fn estimate(
        mosaic: &[f32],
        size: Size2us,
        cfa_type: &CfaType,
        excluded: impl Fn(usize) -> bool,
    ) -> ArrayVec<f32, 3> {
        Self::estimate_sampled(mosaic, size, cfa_type, excluded, MAX_STATISTIC_SAMPLES)
    }

    /// [`Self::estimate`] over the pairs of the rows that keep each colour to at most about
    /// `per_colour` pairs.
    fn estimate_sampled(
        mosaic: &[f32],
        size: Size2us,
        cfa_type: &CfaType,
        excluded: impl Fn(usize) -> bool,
        per_colour: usize,
    ) -> ArrayVec<f32, 3> {
        let mut differences: ArrayVec<Vec<f32>, 3> =
            pair_counts(size, cfa_type, &excluded, per_colour)
                .into_iter()
                .map(Vec::with_capacity)
                .collect();
        for_each_pair(size, cfa_type, &excluded, per_colour, |pair| {
            differences[pair.colour].push(pair.difference(mosaic, size));
        });
        differences.iter_mut().map(|colour| sigma(colour)).collect()
    }

    /// Each colour's background under the flat `gain` divided `mosaic` by: the noise measured over
    /// each of the colour's gain bins on its own, a pair at the mean of its two photosites' gain,
    /// and fitted by [`BackgroundSplit::fit`].
    pub(crate) fn estimate_split(
        mosaic: &[f32],
        size: Size2us,
        cfa_type: &CfaType,
        excluded: impl Fn(usize) -> bool,
        gain: &FlatGain,
    ) -> ArrayVec<BackgroundSplit, 3> {
        let mut binned: ArrayVec<BinnedSamples<Vec<f32>>, 3> =
            pair_counts(size, cfa_type, &excluded, MAX_STATISTIC_SAMPLES)
                .into_iter()
                .map(BinnedSamples::with_capacity)
                .collect();
        for_each_pair(size, cfa_type, &excluded, MAX_STATISTIC_SAMPLES, |pair| {
            let colour = pair.colour;
            let y = pair.y as f32;
            let at = f32::midpoint(
                gain.at(colour, pair.x as f32, y),
                gain.at(colour, pair.neighbour as f32, y),
            );
            binned[colour].push(gain.bins(colour), at, pair.difference(mosaic, size));
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
        pair_counts(size, cfa_type, &|_| false, MAX_STATISTIC_SAMPLES)
            .into_iter()
            .map(BinnedSamples::<Vec<f32>>::heap_bytes)
            .sum()
    }
}

/// A photosite and the nearest same-colour photosite to its right.
#[derive(Debug, Clone, Copy)]
struct Pair {
    x: usize,
    neighbour: usize,
    y: usize,
    colour: usize,
}

impl Pair {
    fn difference(self, mosaic: &[f32], size: Size2us) -> f32 {
        let row = &mosaic[self.y * size.width..(self.y + 1) * size.width];
        row[self.x] - row[self.neighbour]
    }
}

/// How many pairs of each colour [`for_each_pair`] visits under `excluded` and `per_colour`.
fn pair_counts(
    size: Size2us,
    cfa_type: &CfaType,
    excluded: &impl Fn(usize) -> bool,
    per_colour: usize,
) -> ArrayVec<usize, 3> {
    let mut counts: ArrayVec<usize, 3> = (0..cfa_type.num_colors()).map(|_| 0).collect();
    for_each_pair(size, cfa_type, excluded, per_colour, |pair| {
        counts[pair.colour] += 1;
    });
    counts
}

/// Every pair of the rows [`CfaType::sampled_rows`] takes of a `size` mosaic for `per_colour`
/// samples a colour; pairs touching a pixel `excluded` names are skipped.
fn for_each_pair(
    size: Size2us,
    cfa_type: &CfaType,
    excluded: &impl Fn(usize) -> bool,
    per_colour: usize,
    mut visit: impl FnMut(Pair),
) {
    for y in cfa_type.sampled_rows(size, per_colour) {
        for x in 0..size.width {
            let colour = cfa_type.color_at(Vec2us::new(x, y));
            let Some(neighbour) = (x + 1..(x + 1 + MAX_REACH).min(size.width))
                .find(|&nx| cfa_type.color_at(Vec2us::new(nx, y)) == colour)
            else {
                continue;
            };
            if excluded(y * size.width + x) || excluded(y * size.width + neighbour) {
                continue;
            }
            visit(Pair {
                x,
                neighbour,
                y,
                colour,
            });
        }
    }
}

/// The white noise σ of `differences`, each the difference of two independent samples, from their
/// MAD; `0` for fewer than two. Reorders `differences`.
fn sigma(differences: &mut [f32]) -> f32 {
    if differences.len() < 2 {
        return 0.0;
    }
    let centre = median_mut(differences);
    for value in differences.iter_mut() {
        *value = (*value - centre).abs();
    }
    mad_to_sigma(median_mut(differences)) / SQRT_2
}

#[cfg(test)]
mod tests {
    use crate::image::cfa::cfa_pattern::CfaPattern;
    use crate::image::cfa::cfa_type::CfaType;
    use crate::internals::prelude::*;
    use crate::math::noise::difference_noise::DifferenceNoise;
    use crate::math::statistics::subsample::MAX_STATISTIC_SAMPLES;

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
        let mosaic: Vec<f32> = (0..size.pixel_count())
            .map(|index| {
                let (x, y) = (index % 512, index / 512);
                let colour = cfa_type.color_at(Vec2us::new(x, y));
                0.2 + 0.3 * (x as f32 / 512.0) + sigmas[colour] * rng.next_gaussian_f32()
            })
            .collect();
        for per_colour in [MAX_STATISTIC_SAMPLES, 43_690] {
            let estimates =
                DifferenceNoise::estimate_sampled(&mosaic, size, &cfa_type, |_| false, per_colour);
            for (colour, (&estimate, &sigma)) in estimates.iter().zip(&sigmas).enumerate() {
                assert!(
                    (estimate - sigma).abs() < 0.02 * sigma,
                    "{per_colour} a colour, colour {colour}: {estimate}"
                );
            }
        }
    }
}
