//! Sampling one output row at positions already evaluated.
//!
//! Every method reads the same [`SourcePosition`]s — one row of them per output row, evaluated
//! once by [`RowPositions`](crate::registration::resample::row_positions::RowPositions)
//! and shared by every channel and the quality maps. Lanczos runs as a vector kernel, dispatched
//! once per row.

mod simd;

use crate::registration::config::InterpolationMethod;
use crate::registration::resample::kernel;
use crate::registration::resample::kernel::LanczosOrder;
use crate::registration::resample::source_position::SourcePosition;
use imaginarium::Buffer2;

/// Fill `output_row` with `input` sampled by `method` at `positions`, and `border_value` where a
/// position falls outside the source.
pub(super) fn sample_row(
    input: &Buffer2<f32>,
    positions: &[Option<SourcePosition>],
    method: InterpolationMethod,
    border_value: f32,
    output_row: &mut [f32],
) {
    debug_assert_eq!(positions.len(), output_row.len());
    let each = |output_row: &mut [f32], sample: fn(&Buffer2<f32>, SourcePosition) -> f32| {
        for (value, position) in output_row.iter_mut().zip(positions) {
            *value = position.map_or(border_value, |position| sample(input, position));
        }
    };
    match method {
        InterpolationMethod::Nearest => each(output_row, kernel::nearest_sample),
        InterpolationMethod::Bilinear => each(output_row, kernel::bilinear_sample),
        InterpolationMethod::Bicubic => each(output_row, kernel::bicubic_sample),
        InterpolationMethod::Lanczos2 => {
            simd::lanczos_row::<2, 4>(
                LanczosOrder::Two,
                input,
                positions,
                border_value,
                output_row,
            );
        }
        InterpolationMethod::Lanczos3 => {
            simd::lanczos_row::<3, 6>(
                LanczosOrder::Three,
                input,
                positions,
                border_value,
                output_row,
            );
        }
        InterpolationMethod::Lanczos4 => {
            simd::lanczos_row::<4, 8>(
                LanczosOrder::Four,
                input,
                positions,
                border_value,
                output_row,
            );
        }
    }
}

#[cfg(test)]
mod tests;
