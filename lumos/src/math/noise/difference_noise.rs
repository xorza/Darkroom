//! [`DifferenceNoise`]: the white noise of each colour of a CFA mosaic.

use std::f32::consts::SQRT_2;

use arrayvec::ArrayVec;

use crate::io::image::cfa::CfaType;
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
        debug_assert_eq!(mosaic.len(), size.pixel_count());
        let colours = cfa_type.num_colors();
        // Every `row_step`-th row, so each colour keeps at most about `MAX_STATISTIC_SAMPLES`.
        let row_step = (size.pixel_count() / (colours * MAX_STATISTIC_SAMPLES)).max(1);
        let mut differences: ArrayVec<Vec<f32>, 3> = (0..colours).map(|_| Vec::new()).collect();
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
                differences[usize::from(colour)].push(row[x] - row[neighbour]);
            }
        }
        differences
            .into_iter()
            .map(|mut colour| {
                if colour.len() < 2 {
                    return 0.0;
                }
                let centre = median_mut(&mut colour);
                for value in &mut colour {
                    *value = (*value - centre).abs();
                }
                mad_to_sigma(median_mut(&mut colour)) / SQRT_2
            })
            .collect()
    }
}
