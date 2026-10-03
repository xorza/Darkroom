//! Sampling one output row at positions already evaluated.
//!
//! Every method reads the same [`SourcePosition`]s — one row of them per output row, evaluated
//! once by [`RowPositions`](crate::stacking::registration::resample::row_positions::RowPositions)
//! and shared by every channel and the quality maps. Lanczos has a vector backend for its tap
//! weights and its interior accumulation: x86 AVX2/FMA (a 256-bit window for Lanczos3/4, with the
//! weights from an `i32gather` of the table) and aarch64 NEON.

mod simd;

use crate::math::size2us::Size2us;
use crate::stacking::registration::config::InterpolationMethod;
use crate::stacking::registration::resample::kernel;
use crate::stacking::registration::resample::kernel::LanczosOrder;
use crate::stacking::registration::resample::source_position::{SourcePosition, window_inside};
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
            lanczos::<2, 4>(
                LanczosOrder::Two,
                input,
                positions,
                border_value,
                output_row,
            );
        }
        InterpolationMethod::Lanczos3 => {
            lanczos::<3, 6>(
                LanczosOrder::Three,
                input,
                positions,
                border_value,
                output_row,
            );
        }
        InterpolationMethod::Lanczos4 => {
            lanczos::<4, 8>(
                LanczosOrder::Four,
                input,
                positions,
                border_value,
                output_row,
            );
        }
    }
}

/// Lanczos-`A` over each `SIZE = 2A` window: the vector kernel where the window lies inside the
/// source, a scalar loop where only the vector kernel's wider loads would leave it, and
/// edge-extended bilinear where the window itself straddles the border — truncating a signed
/// kernel can leave arbitrarily little weight, which bilinear's clamped taps cannot.
///
/// An interior window is divided by its tap-weight total, which for a full Lanczos window lies
/// within a few per cent of 1 at every fraction, so it needs no guard.
#[expect(
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    reason = "the order A is at most 4, and a window window_inside accepted starts at a non-negative tap"
)]
fn lanczos<const A: usize, const SIZE: usize>(
    order: LanczosOrder,
    input: &Buffer2<f32>,
    positions: &[Option<SourcePosition>],
    border_value: f32,
    output_row: &mut [f32],
) {
    debug_assert_eq!(order.a(), A);
    let lut = order.lut();
    let pixels = input.pixels();
    let width = input.width();
    let size = Size2us::new(width, input.height());
    let reach = A as i32 - 1;

    for (value, position) in output_row.iter_mut().zip(positions) {
        let Some(position) = *position else {
            *value = border_value;
            continue;
        };
        let origin = position.window_origin(reach);
        if !window_inside(origin, SIZE, size) {
            *value = kernel::bilinear_sample(input, position);
            continue;
        }
        let (kx0, ky0) = (origin.x, origin.y);

        let wx = simd::lanczos_weights::<A, SIZE>(lut, position.fx);
        let wy = simd::lanczos_weights::<A, SIZE>(lut, position.fy);
        let inv_total = 1.0 / (wx.iter().sum::<f32>() * wy.iter().sum::<f32>());

        let accumulated = simd::lanczos_accumulate::<SIZE>(input, kx0, ky0, &wx, &wy)
            .unwrap_or_else(|| {
                let (kx, ky) = (kx0 as usize, ky0 as usize);
                let mut sum = 0.0f32;
                for (j, &wyj) in wy.iter().enumerate() {
                    let row = &pixels[(ky + j) * width + kx..][..SIZE];
                    for (&sample, &wxk) in row.iter().zip(&wx) {
                        sum += sample * wxk * wyj;
                    }
                }
                sum
            });
        *value = accumulated * inv_total;
    }
}

#[cfg(test)]
mod tests;
