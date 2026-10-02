//! Vector backends for the Lanczos row warp, and the dispatch between them.
//!
//! An x86 AVX2/FMA kernel and a NEON one, plus an x86 `i32gather` for the tap weights. On x86 the
//! Lanczos3/4 (SIZE=6/8) kernel is 256-bit — one `__m256` load and accumulate per row — while
//! SIZE=4 and every NEON kernel are 128-bit.

use crate::simd::dispatch;
#[cfg(target_arch = "x86_64")]
use crate::stacking::registration::resample::kernel::LANCZOS_LUT_RESOLUTION;
use crate::stacking::registration::resample::kernel::LanczosLut;
use imaginarium::Buffer2;

#[cfg(target_arch = "aarch64")]
mod neon;

#[cfg(target_arch = "x86_64")]
mod x86;

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
        scalar => lut.weights::<SIZE>(frac),
    }
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

#[cfg(test)]
mod tests;
