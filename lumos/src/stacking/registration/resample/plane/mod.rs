//! Single-plane inverse-mapped resampling.

use rayon::prelude::*;

use crate::stacking::registration::config::{InterpolationMethod, WarpParams};
use crate::stacking::registration::resample::{kernel, row};
use crate::stacking::registration::transform::WarpTransform;
use imaginarium::Buffer2;

#[cfg(test)]
mod tests;

pub(crate) fn warp(
    input: &Buffer2<f32>,
    output: &mut Buffer2<f32>,
    transform: &WarpTransform,
    params: &WarpParams,
) {
    let width = input.width();
    let height = input.height();
    debug_assert_eq!(width, output.width());
    debug_assert_eq!(height, output.height());

    match params.method {
        InterpolationMethod::Nearest => warp_rows(output, width, |y, output_row| {
            row::sample(y, transform, output_row, params.border_value, |pos| {
                kernel::interpolate_nearest(input, pos, params.border_value)
            });
        }),
        InterpolationMethod::Bilinear => warp_rows(output, width, |y, output_row| {
            row::bilinear(input, output_row, y, transform, params.border_value);
        }),
        InterpolationMethod::Bicubic => warp_rows(output, width, |y, output_row| {
            row::sample(y, transform, output_row, params.border_value, |pos| {
                kernel::interpolate_bicubic(input, pos, params.border_value)
            });
        }),
        InterpolationMethod::Lanczos2
        | InterpolationMethod::Lanczos3
        | InterpolationMethod::Lanczos4 => warp_rows(output, width, |y, output_row| {
            row::lanczos(input, output_row, y, transform, params);
        }),
    }
}

fn warp_rows(
    output: &mut Buffer2<f32>,
    width: usize,
    warp_row: impl Fn(usize, &mut [f32]) + Sync + Send,
) {
    output
        .pixels_mut()
        .par_chunks_mut(width)
        .enumerate()
        .for_each(|(y, output_row)| warp_row(y, output_row));
}
