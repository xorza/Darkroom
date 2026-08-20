//! SIMD implementations for single-row conversion.
//!
//! # Precision
//!
//! Every kernel here is bit-identical to the scalar reference it stands in for,
//! not merely close to it — [`crate::image::conversion`] picks between the two
//! per format pair, so a pair must not change answers depending on which path a
//! build or a CPU happens to take.
//!
//! That is why the widening kernels *divide* by the source type's full-scale
//! value rather than multiplying by a precomputed reciprocal, which would be
//! several times cheaper: `x * (1.0 / 255.0)` is doubly rounded and disagrees
//! with `x / 255.0` for 126 of the 256 byte values, and `x * (1.0 / 65535.0)`
//! for 512 of the 65 536 word values.
//!
//! The luminance kernels face the same temptation from the other side: the Rec.
//! 709 weights sum to 65536, so keeping the weighted sum in 16-bit lanes would
//! mean scaling them down to 8-bit precision. Both arches accumulate into 32-bit
//! lanes instead and carry the reference's own weights — x86 through `madd`,
//! with green split across two lanes because it overflows a signed one, aarch64
//! through the widening `vmlal`, which takes them unsigned and whole.

#![allow(unsafe_op_in_unsafe_fn)]

cfg_x86_64! {
    mod avx;
    mod sse;
}

cfg_aarch64! {
    mod neon;
}

#[cfg(test)]
mod tests;

use crate::common::color_format::ColorFormat;
#[cfg(target_arch = "x86_64")]
use crate::cpu_features;
use crate::image::conversion::scalar::RgbToLuminance;

/// A row conversion kernel: converts one packed source row of `width` pixels
/// into one packed destination row.
///
/// # Safety
/// The running CPU must support the feature the kernel was compiled for.
/// [`row_converter`] is what establishes that, and is the only thing that hands
/// one of these out.
type RowConvertFn = unsafe fn(src: &[u8], dst: &mut [u8], width: usize);

/// The luminance of a row's sub-vector tail, straight through the scalar
/// reference, so a tail can never disagree with the vector body it follows.
/// `CHANNELS` is the source's bytes per pixel: four for `RGBA`, three for `RGB`.
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
fn luma_tail<const CHANNELS: usize>(src: &[u8], dst: &mut [u8]) {
    let (pixels, _) = src.as_chunks::<CHANNELS>();
    for (pixel, out) in pixels.iter().zip(dst) {
        *out = u8::luminance(pixel[0], pixel[1], pixel[2]);
    }
}

/// The SIMD row kernel for a format pair, or `None` when this build and CPU have
/// no vector path for it — the caller then takes the scalar reference.
// The parameters are read only by the arch-gated tables below; on other targets
// those reads vanish under cfg and the bindings look unused.
#[cfg_attr(
    not(any(target_arch = "x86_64", target_arch = "aarch64")),
    allow(unused_variables)
)]
pub(super) fn row_converter(from: ColorFormat, to: ColorFormat) -> Option<RowConvertFn> {
    #[cfg(target_arch = "x86_64")]
    return x86_64_converter(from, to);

    #[cfg(target_arch = "aarch64")]
    return aarch64_converter(from, to);

    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    None
}

cfg_x86_64! {
    /// The x86_64 table. Channel-count changes shuffle bytes and so need SSSE3;
    /// everything below that falls to the scalar reference, which is also where
    /// a CPU without SSE2 lands for every pair.
    fn x86_64_converter(from: ColorFormat, to: ColorFormat) -> Option<RowConvertFn> {
        let features = cpu_features::get();
        if !features.sse2 {
            return None;
        }
        match (from, to) {
            (ColorFormat::RGBA_U8, ColorFormat::RGB_U8) if features.ssse3 => {
                Some(if features.avx2 {
                    avx::convert_rgba_to_rgb_row_avx2 as RowConvertFn
                } else {
                    sse::convert_rgba_to_rgb_row_ssse3 as RowConvertFn
                })
            }
            (ColorFormat::RGB_U8, ColorFormat::RGBA_U8) if features.ssse3 => {
                Some(sse::convert_rgb_to_rgba_row_ssse3 as RowConvertFn)
            }
            (ColorFormat::RGBA_U8, ColorFormat::L_U8) if features.ssse3 => {
                Some(sse::convert_rgba_to_l_row_ssse3 as RowConvertFn)
            }
            (ColorFormat::RGB_U8, ColorFormat::L_U8) if features.ssse3 => {
                Some(sse::convert_rgb_to_l_row_ssse3 as RowConvertFn)
            }
            (ColorFormat::L_U8, ColorFormat::RGBA_U8) if features.ssse3 => {
                Some(sse::convert_l_to_rgba_row_ssse3 as RowConvertFn)
            }
            (ColorFormat::L_U8, ColorFormat::RGB_U8) if features.ssse3 => {
                Some(sse::convert_l_to_rgb_row_ssse3 as RowConvertFn)
            }
            _ => elem_converter(from, to),
        }
    }
}

cfg_aarch64! {
    /// The aarch64 table. NEON is baseline, so every pair with a kernel has it.
    fn aarch64_converter(from: ColorFormat, to: ColorFormat) -> Option<RowConvertFn> {
        match (from, to) {
            (ColorFormat::RGBA_U8, ColorFormat::RGB_U8) => {
                Some(neon::convert_rgba_to_rgb_row_neon as RowConvertFn)
            }
            (ColorFormat::RGB_U8, ColorFormat::RGBA_U8) => {
                Some(neon::convert_rgb_to_rgba_row_neon as RowConvertFn)
            }
            (ColorFormat::RGBA_U8, ColorFormat::L_U8) => {
                Some(neon::convert_rgba_to_l_row_neon as RowConvertFn)
            }
            (ColorFormat::RGB_U8, ColorFormat::L_U8) => {
                Some(neon::convert_rgb_to_l_row_neon as RowConvertFn)
            }
            (ColorFormat::L_U8, ColorFormat::RGBA_U8) => {
                Some(neon::convert_l_to_rgba_row_neon as RowConvertFn)
            }
            (ColorFormat::L_U8, ColorFormat::RGB_U8) => {
                Some(neon::convert_l_to_rgb_row_neon as RowConvertFn)
            }
            _ => elem_converter(from, to),
        }
    }
}

/// The kernel for an element conversion — a change of sample type at an
/// unchanged channel count. One table serves both arches: each selector below
/// resolves its own backend, so nothing here is arch-specific.
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
fn elem_converter(from: ColorFormat, to: ColorFormat) -> Option<RowConvertFn> {
    match (from, to) {
        (ColorFormat::RGBA_F32, ColorFormat::RGBA_U8)
        | (ColorFormat::RGB_F32, ColorFormat::RGB_U8)
        | (ColorFormat::L_F32, ColorFormat::L_U8) => f32_to_u8(),

        (ColorFormat::RGBA_U8, ColorFormat::RGBA_F32)
        | (ColorFormat::RGB_U8, ColorFormat::RGB_F32)
        | (ColorFormat::L_U8, ColorFormat::L_F32) => u8_to_f32(),

        (ColorFormat::RGBA_U8, ColorFormat::RGBA_U16)
        | (ColorFormat::RGB_U8, ColorFormat::RGB_U16)
        | (ColorFormat::L_U8, ColorFormat::L_U16) => u8_to_u16(),

        (ColorFormat::RGBA_U16, ColorFormat::RGBA_U8)
        | (ColorFormat::RGB_U16, ColorFormat::RGB_U8)
        | (ColorFormat::L_U16, ColorFormat::L_U8) => u16_to_u8(),

        (ColorFormat::RGBA_U16, ColorFormat::RGBA_F32)
        | (ColorFormat::RGB_U16, ColorFormat::RGB_F32)
        | (ColorFormat::L_U16, ColorFormat::L_F32) => u16_to_f32(),

        (ColorFormat::RGBA_F32, ColorFormat::RGBA_U16)
        | (ColorFormat::RGB_F32, ColorFormat::RGB_U16)
        | (ColorFormat::L_F32, ColorFormat::L_U16) => f32_to_u16(),

        _ => None,
    }
}

/// One backend's byte-slice entry point for an element conversion: casts the row
/// to the typed slices `$core` takes, then forwards.
///
/// Element conversion is per-sample and so channel-agnostic — L, RGB and RGBA
/// differ only in how many samples a row holds, and that is
/// `dst.len() / size_of::<$dst>()` because each row handed to a kernel is
/// exactly one packed row. Which is why `width` goes unread here.
macro_rules! elem_kernel {
    ($name:ident, $core:path, $src:ty => $dst:ty) => {
        unsafe fn $name(src: &[u8], dst: &mut [u8], _width: usize) {
            let count = dst.len() / size_of::<$dst>();
            let src: &[$src] = bytemuck::cast_slice(&src[..count * size_of::<$src>()]);
            let dst: &mut [$dst] = bytemuck::cast_slice_mut(dst);
            // SAFETY: forwarded from this kernel's own contract.
            unsafe { $core(src, dst) };
        }
    };
}

/// Defines `$name`, which resolves the widest backend the running CPU offers for
/// one element conversion — or `None` when it offers none, leaving the caller
/// its scalar reference.
///
/// The feature test lives here, once, rather than at each of the six type pairs;
/// `$sse_feature` names the `X86Features` field the SSE backend needs, since the
/// SSE4.1 kernels are not reachable on an SSE2-only CPU.
macro_rules! elem_converter {
    (
        $name:ident: $src:ty => $dst:ty,
        avx2: $avx2:path,
        $sse_feature:ident: $sse:path,
        neon: $neon:path $(,)?
    ) => {
        cfg_x86_64! {
            fn $name() -> Option<RowConvertFn> {
                elem_kernel!(wide, $avx2, $src => $dst);
                elem_kernel!(narrow, $sse, $src => $dst);

                let features = cpu_features::get();
                if features.avx2 {
                    Some(wide as RowConvertFn)
                } else if features.$sse_feature {
                    Some(narrow as RowConvertFn)
                } else {
                    None
                }
            }
        }

        cfg_aarch64! {
            fn $name() -> Option<RowConvertFn> {
                elem_kernel!(kernel, $neon, $src => $dst);
                Some(kernel as RowConvertFn)
            }
        }
    };
}

elem_converter!(
    f32_to_u8: f32 => u8,
    avx2: avx::convert_f32_to_u8_row_avx2,
    sse2: sse::convert_f32_to_u8_row_sse2,
    neon: neon::convert_f32_to_u8_row_neon,
);

elem_converter!(
    u8_to_f32: u8 => f32,
    avx2: avx::convert_u8_to_f32_row_avx2,
    sse2: sse::convert_u8_to_f32_row_sse2,
    neon: neon::convert_u8_to_f32_row_neon,
);

elem_converter!(
    u8_to_u16: u8 => u16,
    avx2: avx::convert_u8_to_u16_row_avx2,
    sse2: sse::convert_u8_to_u16_row_sse2,
    neon: neon::convert_u8_to_u16_row_neon,
);

elem_converter!(
    u16_to_u8: u16 => u8,
    avx2: avx::convert_u16_to_u8_row_avx2,
    sse2: sse::convert_u16_to_u8_row_sse2,
    neon: neon::convert_u16_to_u8_row_neon,
);

elem_converter!(
    u16_to_f32: u16 => f32,
    avx2: avx::convert_u16_to_f32_row_avx2,
    sse2: sse::convert_u16_to_f32_row_sse2,
    neon: neon::convert_u16_to_f32_row_neon,
);

elem_converter!(
    f32_to_u16: f32 => u16,
    avx2: avx::convert_f32_to_u16_row_avx2,
    sse4_1: sse::convert_f32_to_u16_row_sse41,
    neon: neon::convert_f32_to_u16_row_neon,
);
