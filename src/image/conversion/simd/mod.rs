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
#[cfg(target_arch = "x86_64")]
use crate::cpu_features;

/// A row conversion kernel: converts one packed source row of `width` pixels
/// into one packed destination row.
///
/// # Safety
/// The running CPU must support the feature the kernel was compiled for.
/// [`row_converter`] is what establishes that, and is the only thing that hands
/// one of these out.
pub(crate) type RowConvertFn = unsafe fn(src: &[u8], dst: &mut [u8], width: usize);

/// The `x86_64` and aarch64 tiers a conversion can run on. The tests sweep every tier the host
/// supports; dispatch takes the widest.
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tier {
    #[cfg(target_arch = "x86_64")]
    Sse2,
    #[cfg(target_arch = "x86_64")]
    Ssse3,
    #[cfg(target_arch = "x86_64")]
    Sse41,
    #[cfg(target_arch = "x86_64")]
    Avx2,
    #[cfg(target_arch = "aarch64")]
    Neon,
}

#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
impl Tier {
    /// Every tier, narrowest first.
    #[cfg(target_arch = "x86_64")]
    pub(crate) const ALL: [Self; 4] = [Self::Sse2, Self::Ssse3, Self::Sse41, Self::Avx2];
    /// Every tier.
    #[cfg(target_arch = "aarch64")]
    pub(crate) const ALL: [Self; 1] = [Self::Neon];

    /// Whether the running CPU has this tier.
    pub(crate) fn is_supported(self) -> bool {
        match self {
            #[cfg(target_arch = "x86_64")]
            Self::Sse2 => true,
            #[cfg(target_arch = "x86_64")]
            Self::Ssse3 => cpu_features::get().ssse3,
            #[cfg(target_arch = "x86_64")]
            Self::Sse41 => cpu_features::get().sse4_1,
            #[cfg(target_arch = "x86_64")]
            Self::Avx2 => cpu_features::get().avx2,
            #[cfg(target_arch = "aarch64")]
            Self::Neon => true,
        }
    }

    /// The widest tier the running CPU has.
    fn widest() -> Option<Self> {
        Self::ALL.into_iter().rev().find(|tier| tier.is_supported())
    }
}

/// The SIMD row kernel for a format pair, or `None` when this build and CPU have
/// no vector path for it — the caller then takes the scalar reference.
pub(crate) fn row_converter(from: ColorFormat, to: ColorFormat) -> Option<RowConvertFn> {
    #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
    return tier_converter(Tier::widest()?, from, to);

    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        let _ = (from, to);
        None
    }
}

/// The kernel `tier` has for a format pair, or the one the next narrower tier has — what
/// dispatch would pick on a CPU whose widest tier is `tier`.
///
/// # Safety (of the returned kernel)
/// The caller must have checked [`Tier::is_supported`] for `tier`.
#[cfg(target_arch = "x86_64")]
pub(crate) fn tier_converter(
    tier: Tier,
    from: ColorFormat,
    to: ColorFormat,
) -> Option<RowConvertFn> {
    let kernel = match (tier, from, to) {
        (Tier::Avx2, ColorFormat::RGBA_U8, ColorFormat::RGB_U8) => {
            avx::convert_rgba_to_rgb_row_avx2 as RowConvertFn
        }
        (Tier::Ssse3 | Tier::Sse41, ColorFormat::RGBA_U8, ColorFormat::RGB_U8) => {
            sse::convert_rgba_to_rgb_row_ssse3
        }
        (Tier::Ssse3 | Tier::Sse41 | Tier::Avx2, ColorFormat::RGB_U8, ColorFormat::RGBA_U8) => {
            sse::convert_rgb_to_rgba_row_ssse3
        }
        (Tier::Ssse3 | Tier::Sse41 | Tier::Avx2, ColorFormat::RGBA_U8, ColorFormat::L_U8) => {
            sse::convert_rgba_to_l_row_ssse3
        }
        (Tier::Ssse3 | Tier::Sse41 | Tier::Avx2, ColorFormat::RGB_U8, ColorFormat::L_U8) => {
            sse::convert_rgb_to_l_row_ssse3
        }
        (Tier::Ssse3 | Tier::Sse41 | Tier::Avx2, ColorFormat::L_U8, ColorFormat::RGBA_U8) => {
            sse::convert_l_to_rgba_row_ssse3
        }
        (Tier::Ssse3 | Tier::Sse41 | Tier::Avx2, ColorFormat::L_U8, ColorFormat::RGB_U8) => {
            sse::convert_l_to_rgb_row_ssse3
        }
        _ => return element_converter(tier, from, to),
    };
    Some(kernel)
}

/// The aarch64 table. NEON is baseline, so every pair with a kernel has it.
#[cfg(target_arch = "aarch64")]
pub(crate) fn tier_converter(
    tier: Tier,
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

/// The kernel for an element conversion — a change of sample type at an
/// unchanged channel count, which is per-sample and so channel-agnostic.
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
fn element_converter(tier: Tier, from: ColorFormat, to: ColorFormat) -> Option<RowConvertFn> {
    use crate::common::color_format::SampleType::{F32, U8, U16};

    if from.channel_count != to.channel_count {
        return None;
    }
    #[cfg(target_arch = "x86_64")]
    let kernel = match (tier, from.sample_type, to.sample_type) {
        (Tier::Avx2, F32, U8) => avx::convert_f32_to_u8_row_avx2 as RowConvertFn,
        (Tier::Avx2, U8, F32) => avx::convert_u8_to_f32_row_avx2,
        (Tier::Avx2, U8, U16) => avx::convert_u8_to_u16_row_avx2,
        (Tier::Avx2, U16, U8) => avx::convert_u16_to_u8_row_avx2,
        (Tier::Avx2, U16, F32) => avx::convert_u16_to_f32_row_avx2,
        (Tier::Avx2, F32, U16) => avx::convert_f32_to_u16_row_avx2,
        (_, F32, U8) => sse::convert_f32_to_u8_row_sse2,
        (_, U8, F32) => sse::convert_u8_to_f32_row_sse2,
        (_, U8, U16) => sse::convert_u8_to_u16_row_sse2,
        (_, U16, U8) => sse::convert_u16_to_u8_row_sse2,
        (_, U16, F32) => sse::convert_u16_to_f32_row_sse2,
        (Tier::Sse41, F32, U16) => sse::convert_f32_to_u16_row_sse41,
        _ => return None,
    };
    #[cfg(target_arch = "aarch64")]
    let kernel = match (tier, from.sample_type, to.sample_type) {
        (Tier::Neon, F32, U8) => neon::convert_f32_to_u8_row_neon as RowConvertFn,
        (Tier::Neon, U8, F32) => neon::convert_u8_to_f32_row_neon,
        (Tier::Neon, U8, U16) => neon::convert_u8_to_u16_row_neon,
        (Tier::Neon, U16, U8) => neon::convert_u16_to_u8_row_neon,
        (Tier::Neon, U16, F32) => neon::convert_u16_to_f32_row_neon,
        (Tier::Neon, F32, U16) => neon::convert_f32_to_u16_row_neon,
        _ => return None,
    };
    Some(kernel)
}

/// One packed row seen as the element types an element kernel converts between.
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
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
