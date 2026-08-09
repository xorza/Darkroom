//! Vector backends for the row-warp kernels, and the dispatch between them.
//!
//! Bilinear has an AVX2, an SSE4.1 and a NEON kernel; Lanczos has an x86 AVX2/FMA one and a NEON
//! one, plus an x86 `i32gather` for the tap weights. On x86 the Lanczos3/4 (SIZE=6/8) kernel is
//! 256-bit — one `__m256` load and accumulate per row — while SIZE=4 and every NEON kernel are
//! 128-bit.

#[cfg(target_arch = "aarch64")]
use crate::simd::NEON_F32_LANES;
use crate::simd::dispatch;
#[cfg(target_arch = "x86_64")]
use crate::simd::{AVX2_F32_LANES, SSE_F32_LANES};
#[cfg(target_arch = "x86_64")]
use crate::stacking::registration::resample::kernel::LANCZOS_LUT_RESOLUTION;
use crate::stacking::registration::resample::kernel::LanczosLut;
use crate::stacking::registration::transform::Transform;
use imaginarium::Buffer2;

#[cfg(target_arch = "aarch64")]
mod neon;

#[cfg(target_arch = "x86_64")]
mod x86;

/// Warp one row with a vector bilinear kernel, reporting whether it ran.
///
/// `false` when the target has no backend or the row is shorter than one vector — nothing has been
/// written and the caller takes the scalar path. Each kernel consumes one vector of output pixels
/// per chunk (`output_width / LANES` chunks), so a shorter row gives the chunk loop nothing to do
/// and every pixel falls through to the kernel's own remainder handling; cheaper to go scalar. A
/// structural minimum, not a measured crossover — these kernels win from one full vector up.
///
/// Every kernel hardcodes a 0.0 border, so the caller checks `border_value` before calling.
pub(super) fn bilinear(
    input: &Buffer2<f32>,
    output_row: &mut [f32],
    output_y: usize,
    transform: &Transform,
) -> bool {
    dispatch! {
        x86: avx2 if output_row.len() >= AVX2_F32_LANES => {
            x86::bilinear_avx2(input, output_row, output_y, transform);
            true
        },
        x86: sse4_1 if output_row.len() >= SSE_F32_LANES => {
            x86::bilinear_sse(input, output_row, output_y, transform);
            true
        },
        aarch64 if output_row.len() >= NEON_F32_LANES => {
            neon::bilinear_neon(input, output_row, output_y, transform);
            true
        },
        scalar => false,
    }
}

/// The `SIZE` separable Lanczos tap weights for fractional offset `frac`.
///
/// The gather kernel only pays off once ≥ 6 taps amortize its 8-wide gather, so Lanczos2 stays
/// scalar — the gather measured ~6% slower there. `SIZE > 4` is const, so the guard folds away.
#[inline]
pub(super) fn lanczos_weights<const A: usize, const SIZE: usize>(
    lut: &LanczosLut,
    frac: f32,
) -> [f32; SIZE] {
    dispatch! {
        x86: avx2_fma if SIZE > 4 => x86::lanczos_weights_gather::<A, SIZE>(
            lut.values.as_ptr(),
            LANCZOS_LUT_RESOLUTION as f32,
            frac,
        ),
        scalar => lanczos_weights_scalar::<A, SIZE>(lut, frac),
    }
}

/// Scalar Lanczos tap weights for fractional offset `frac` (non-x86 / no-AVX2 fallback for
/// [`x86::lanczos_weights_gather`]). Same distance convention as the gather helper.
///
/// For `i < A` the distance is `(A-1-i) + frac ∈ [0, A)`; for `i ≥ A` it is `(i-A+1) - frac ∈
/// (0, A]` — both non-negative, hence `lookup_positive`.
#[inline]
fn lanczos_weights_scalar<const A: usize, const SIZE: usize>(
    lut: &LanczosLut,
    frac: f32,
) -> [f32; SIZE] {
    let a_minus_1 = A as i32 - 1;
    let mut w = [0.0f32; SIZE];
    for (i, wi) in w.iter_mut().enumerate() {
        *wi = if i < A {
            lut.lookup_positive((a_minus_1 - i as i32) as f32 + frac)
        } else {
            lut.lookup_positive((i as i32 - a_minus_1) as f32 - frac)
        };
    }
    w
}

/// Vector accumulation of the `SIZE`×`SIZE` weighted window whose top-left tap is `(kx0, ky0)`.
///
/// `None` when the target has no vector backend, or when the window's loads would leave the
/// image — the SIMD bounds are wider than the scalar loop's, so the caller has to re-test before
/// taking its own fast path. x86 loads 8 floats per row for Lanczos3/4 (SIZE=6 zero-pads two) and
/// NEON walks that same 8-wide window as a 128-bit lo+hi pair, so both need `kx0 + 8 ≤ width`
/// where the scalar loop needs only `kx0 + SIZE ≤ width`.
#[inline]
pub(super) fn lanczos_accumulate<const SIZE: usize>(
    input: &Buffer2<f32>,
    kx0: i32,
    ky0: i32,
    wx: &[f32; SIZE],
    wy: &[f32; SIZE],
) -> Option<f32> {
    let simd_cols: i32 = if SIZE > 4 { 8 } else { SIZE as i32 };
    if kx0 < 0
        || ky0 < 0
        || kx0 + simd_cols > input.width() as i32
        || ky0 + SIZE as i32 > input.height() as i32
    {
        return None;
    }

    let pixels = input.pixels();
    let width = input.width();
    let kx = kx0 as usize;
    let ky = ky0 as usize;

    dispatch! {
        x86: avx2_fma => Some(x86::lanczos_kernel_fma::<SIZE>(pixels, width, kx, ky, wx, wy)),
        aarch64 => Some(neon::lanczos_kernel_neon::<SIZE>(pixels, width, kx, ky, wx, wy)),
        scalar => None,
    }
}
