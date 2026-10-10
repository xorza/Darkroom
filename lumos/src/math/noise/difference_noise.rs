//! [`DifferenceNoise`]: the white noise of each colour of a CFA mosaic.

use std::f32::consts::SQRT_2;

use arrayvec::ArrayVec;

use crate::io::image::cfa::CfaType;
use crate::io::image::flat_gain::FlatGain;
use crate::math::noise::background_split::{BackgroundSplit, BinnedSamples};
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
        let mut differences: ArrayVec<Vec<f32>, 3> =
            (0..cfa_type.num_colors()).map(|_| Vec::new()).collect();
        for_each_pair(mosaic, size, cfa_type, &excluded, |pair| {
            differences[usize::from(pair.colour)].push(pair.difference);
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
        let mut binned: ArrayVec<BinnedSamples<f32>, 3> = (0..cfa_type.num_colors())
            .map(|_| BinnedSamples::new())
            .collect();
        for_each_pair(mosaic, size, cfa_type, &excluded, |pair| {
            let colour = usize::from(pair.colour);
            let y = pair.y as f32;
            let at = f32::midpoint(
                gain.at(colour, pair.x as f32, y),
                gain.at(colour, pair.neighbour as f32, y),
            );
            binned[colour].push(gain.bins(colour), at, pair.difference);
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
}

/// One difference of a photosite and the nearest same-colour photosite to its right.
#[derive(Debug, Clone, Copy)]
struct Pair {
    x: usize,
    neighbour: usize,
    y: usize,
    colour: u8,
    difference: f32,
}

/// Every pair of every `row_step`-th row, so each colour keeps at most about
/// `MAX_STATISTIC_SAMPLES`; pairs touching a pixel `excluded` names are skipped.
fn for_each_pair(
    mosaic: &[f32],
    size: Size2us,
    cfa_type: &CfaType,
    excluded: &impl Fn(usize) -> bool,
    mut visit: impl FnMut(Pair),
) {
    debug_assert_eq!(mosaic.len(), size.pixel_count());
    let row_step = (size.pixel_count() / (cfa_type.num_colors() * MAX_STATISTIC_SAMPLES)).max(1);
    for y in (0..size.height).step_by(row_step) {
        let row = &mosaic[y * size.width..(y + 1) * size.width];
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
                difference: row[x] - row[neighbour],
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
