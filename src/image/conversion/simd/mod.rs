//! SIMD implementations for single-row conversion.
//!
//! # Precision
//!
//! Every kernel here is bit-identical to the scalar reference it stands in for,
//! not merely close to it — [`crate::image::conversion`] picks between the two
//! per format pair, so a pair must not change answers depending on which path a
//! build or a CPU happens to take. Every kernel hands its sub-vector tail to the
//! reference itself.
//!
//! That is why the widening kernels *divide* by the source type's full-scale
//! value rather than multiplying by a precomputed reciprocal, which would be
//! several times cheaper: `x * (1.0 / 255.0)` is doubly rounded and disagrees
//! with `x / 255.0` for 126 of the 256 byte values, and `x * (1.0 / 65535.0)`
//! for 512 of the 65 536 word values.
//!
//! For the same reason the float narrowing kernels multiply in `f64`, as the
//! reference does: in `f32`, `x · 255` rounds onto `k + ½` for 128 inputs on
//! `[0, 1]` whose true product lies just past the tie, and `x · 65535` for
//! 32 640, and ties-to-even then lands on the wrong side. The `u16 → u8` kernels
//! compute `round(v / 257)` exactly in integers.
//!
//! The luminance kernels face the same temptation from the other side: the Rec.
//! 709 weights sum to 65536, so keeping the weighted sum in 16-bit lanes would
//! mean scaling them down to 8-bit precision. Both arches accumulate into 32-bit
//! lanes instead and carry the reference's own weights — x86 through `madd`,
//! with green split across two lanes because it overflows a signed one, aarch64
//! through the widening `vmlal`, which takes them unsigned and whole — and round
//! the sum as the reference does.

#[cfg(target_arch = "x86_64")]
mod avx;
#[cfg(target_arch = "aarch64")]
mod neon;
#[cfg(target_arch = "x86_64")]
mod sse;

use crate::common::color_format::ColorFormat;
use crate::common::sample::Sample;
use crate::simd_tier::SimdTier;

/// A row conversion kernel: converts one packed source row of `width` pixels
/// into one packed destination row.
///
/// # Safety
/// The running CPU must support the feature the kernel was compiled for.
/// [`row_converter`] is what establishes that, and is the only thing that hands
/// one of these out.
pub(crate) type RowConvertFn = unsafe fn(src: &[u8], dst: &mut [u8], width: usize);

/// The SIMD row kernel for a format pair, or `None` when this build and CPU have
/// no vector path for it — the caller then takes the scalar reference.
pub(crate) fn row_converter(from: ColorFormat, to: ColorFormat) -> Option<RowConvertFn> {
    tier_converter(SimdTier::widest()?, from, to)
}

/// The kernel `tier` has for a format pair, or the one the next narrower tier has — what
/// dispatch would pick on a CPU whose widest tier is `tier`.
///
/// # Safety (of the returned kernel)
/// The caller must have checked [`SimdTier::is_supported`] for `tier`.
#[cfg(target_arch = "x86_64")]
pub(crate) fn tier_converter(
    tier: SimdTier,
    from: ColorFormat,
    to: ColorFormat,
) -> Option<RowConvertFn> {
    // No conversion fuses a multiply-add, so the FMA tier runs the AVX2 kernels.
    let tier = tier.min(SimdTier::Avx2);
    let kernel = match (tier, from, to) {
        (SimdTier::Avx2, ColorFormat::RGBA_U8, ColorFormat::RGB_U8) => {
            avx::convert_rgba_to_rgb_row_avx2 as RowConvertFn
        }
        (SimdTier::Ssse3 | SimdTier::Sse41, ColorFormat::RGBA_U8, ColorFormat::RGB_U8) => {
            sse::convert_rgba_to_rgb_row_ssse3
        }
        (
            SimdTier::Ssse3 | SimdTier::Sse41 | SimdTier::Avx2,
            ColorFormat::RGB_U8,
            ColorFormat::RGBA_U8,
        ) => sse::convert_rgb_to_rgba_row_ssse3,
        (
            SimdTier::Ssse3 | SimdTier::Sse41 | SimdTier::Avx2,
            ColorFormat::RGBA_U8,
            ColorFormat::L_U8,
        ) => sse::convert_rgba_to_l_row_ssse3,
        (
            SimdTier::Ssse3 | SimdTier::Sse41 | SimdTier::Avx2,
            ColorFormat::RGB_U8,
            ColorFormat::L_U8,
        ) => sse::convert_rgb_to_l_row_ssse3,
        (
            SimdTier::Ssse3 | SimdTier::Sse41 | SimdTier::Avx2,
            ColorFormat::L_U8,
            ColorFormat::RGBA_U8,
        ) => sse::convert_l_to_rgba_row_ssse3,
        (
            SimdTier::Ssse3 | SimdTier::Sse41 | SimdTier::Avx2,
            ColorFormat::L_U8,
            ColorFormat::RGB_U8,
        ) => sse::convert_l_to_rgb_row_ssse3,
        _ => return element_converter(tier, from, to),
    };
    Some(kernel)
}

/// The aarch64 table. NEON is baseline, so every pair with a kernel has it.
#[cfg(target_arch = "aarch64")]
pub(crate) fn tier_converter(
    tier: SimdTier,
    from: ColorFormat,
    to: ColorFormat,
) -> Option<RowConvertFn> {
    let kernel = match (from, to) {
        (ColorFormat::RGBA_U8, ColorFormat::RGB_U8) => {
            neon::convert_rgba_to_rgb_row_neon as RowConvertFn
        }
        (ColorFormat::RGB_U8, ColorFormat::RGBA_U8) => neon::convert_rgb_to_rgba_row_neon,
        (ColorFormat::RGBA_U8, ColorFormat::L_U8) => neon::convert_rgba_to_l_row_neon,
        (ColorFormat::RGB_U8, ColorFormat::L_U8) => neon::convert_rgb_to_l_row_neon,
        (ColorFormat::L_U8, ColorFormat::RGBA_U8) => neon::convert_l_to_rgba_row_neon,
        (ColorFormat::L_U8, ColorFormat::RGB_U8) => neon::convert_l_to_rgb_row_neon,
        _ => return element_converter(tier, from, to),
    };
    Some(kernel)
}

/// No arch kernels: [`SimdTier`] has no value here.
#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
pub(crate) fn tier_converter(
    tier: SimdTier,
    _from: ColorFormat,
    _to: ColorFormat,
) -> Option<RowConvertFn> {
    match tier {}
}

/// The kernel for an element conversion — a change of sample type at an
/// unchanged channel count, which is per-sample and so channel-agnostic.
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
fn element_converter(tier: SimdTier, from: ColorFormat, to: ColorFormat) -> Option<RowConvertFn> {
    use crate::common::color_format::SampleType::{F32, U8, U16};

    if from.channel_count != to.channel_count {
        return None;
    }
    #[cfg(target_arch = "x86_64")]
    let kernel = match (tier, from.sample_type, to.sample_type) {
        (SimdTier::Avx2, F32, U8) => avx::convert_f32_to_u8_row_avx2 as RowConvertFn,
        (SimdTier::Avx2, U8, F32) => avx::convert_u8_to_f32_row_avx2,
        (SimdTier::Avx2, U8, U16) => avx::convert_u8_to_u16_row_avx2,
        (SimdTier::Avx2, U16, U8) => avx::convert_u16_to_u8_row_avx2,
        (SimdTier::Avx2, U16, F32) => avx::convert_u16_to_f32_row_avx2,
        (SimdTier::Avx2, F32, U16) => avx::convert_f32_to_u16_row_avx2,
        (_, F32, U8) => sse::convert_f32_to_u8_row_sse2,
        (_, U8, F32) => sse::convert_u8_to_f32_row_sse2,
        (_, U8, U16) => sse::convert_u8_to_u16_row_sse2,
        (_, U16, U8) => sse::convert_u16_to_u8_row_sse2,
        (_, U16, F32) => sse::convert_u16_to_f32_row_sse2,
        (SimdTier::Sse41, F32, U16) => sse::convert_f32_to_u16_row_sse41,
        _ => return None,
    };
    #[cfg(target_arch = "aarch64")]
    let kernel = match (tier, from.sample_type, to.sample_type) {
        (SimdTier::Neon, F32, U8) => neon::convert_f32_to_u8_row_neon as RowConvertFn,
        (SimdTier::Neon, U8, F32) => neon::convert_u8_to_f32_row_neon,
        (SimdTier::Neon, U8, U16) => neon::convert_u8_to_u16_row_neon,
        (SimdTier::Neon, U16, U8) => neon::convert_u16_to_u8_row_neon,
        (SimdTier::Neon, U16, F32) => neon::convert_u16_to_f32_row_neon,
        (SimdTier::Neon, F32, U16) => neon::convert_f32_to_u16_row_neon,
        _ => return None,
    };
    Some(kernel)
}

/// One packed row seen as the element types an element kernel converts between.
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
#[derive(Debug)]
pub(crate) struct ElementRow<'a, S, D> {
    pub(crate) src: &'a [S],
    pub(crate) dst: &'a mut [D],
}

#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
impl<'a, S: Sample, D: Sample> ElementRow<'a, S, D> {
    /// A row's element count is `dst.len() / size_of::<D>()`: each row handed to a kernel is
    /// exactly one packed row, while `src` runs to the end of the image.
    pub(crate) fn new(src: &'a [u8], dst: &'a mut [u8]) -> Self {
        let count = dst.len() / size_of::<D>();
        Self {
            src: bytemuck::cast_slice(&src[..count * size_of::<S>()]),
            dst: bytemuck::cast_slice_mut(dst),
        }
    }
}

#[cfg(test)]
mod tests;
