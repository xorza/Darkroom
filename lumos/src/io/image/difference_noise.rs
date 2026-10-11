//! [`DifferenceNoise`]: the white noise of each colour of a CFA mosaic.

use std::f32::consts::SQRT_2;

use arrayvec::ArrayVec;

use crate::io::image::cfa::CfaType;
use crate::io::image::flat_gain::FlatGain;
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
        let mut differences: ArrayVec<Vec<f32>, 3> = pair_counts(size, cfa_type, &excluded)
            .into_iter()
            .map(Vec::with_capacity)
            .collect();
        for_each_pair(size, cfa_type, &excluded, |pair| {
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
            pair_counts(size, cfa_type, &excluded)
                .into_iter()
                .map(BinnedSamples::with_capacity)
                .collect();
        for_each_pair(size, cfa_type, &excluded, |pair| {
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
        pair_counts(size, cfa_type, &|_| false)
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

/// How many pairs of each colour [`for_each_pair`] visits under `excluded`.
fn pair_counts(
    size: Size2us,
    cfa_type: &CfaType,
    excluded: &impl Fn(usize) -> bool,
) -> ArrayVec<usize, 3> {
    let mut counts: ArrayVec<usize, 3> = (0..cfa_type.num_colors()).map(|_| 0).collect();
    for_each_pair(size, cfa_type, excluded, |pair| counts[pair.colour] += 1);
    counts
}

/// Every pair of every `row_step`-th row of a `size` mosaic, so each colour keeps at most about
/// `MAX_STATISTIC_SAMPLES`; pairs touching a pixel `excluded` names are skipped.
fn for_each_pair(
    size: Size2us,
    cfa_type: &CfaType,
    excluded: &impl Fn(usize) -> bool,
    mut visit: impl FnMut(Pair),
) {
    let row_step = (size.pixel_count() / (cfa_type.num_colors() * MAX_STATISTIC_SAMPLES)).max(1);
    for y in (0..size.height).step_by(row_step) {
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
    use crate::internals::prelude::*;
    use crate::io::image::cfa::CfaType;
    use crate::io::image::cfa::cfa_pattern::CfaPattern;
    use crate::io::image::difference_noise::DifferenceNoise;

    /// Each colour of a Bayer mosaic reads its own noise, through a gradient the differences
    /// cancel. σ = 0.01 red, 0.02 green, 0.03 blue on a 512² mosaic: red and blue keep 32 768 pairs
    /// each, green 65 536. MAD's standard error is 1.166·σ/√n, so 3 of them is 1.9% of σ for red
    /// and blue and 1.4% for green; 2% of σ covers all three.
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
        let estimates = DifferenceNoise::estimate(&mosaic, size, &cfa_type, |_| false);
        for (colour, (&estimate, &sigma)) in estimates.iter().zip(&sigmas).enumerate() {
            assert!(
                (estimate - sigma).abs() < 0.02 * sigma,
                "colour {colour}: {estimate}"
            );
        }
    }
}
