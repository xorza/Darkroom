#[cfg(target_arch = "aarch64")]
mod neon;
#[cfg(target_arch = "x86_64")]
mod sse41;

use common::{SimdTier, par};
use rayon::prelude::*;

use crate::color::color_format::{ColorFormat, SampleType};
use crate::color::sample::Sample;
use crate::image::InterleavedImage;
use crate::ops::blend::{Blend, BlendMode};

/// A SIMD row kernel blending one `src`/`dst` row pair into `out`. All three are
/// exactly one packed row, so a kernel's pixel count is its slice length and it
/// never has cause to read past the row it was given.
///
/// # Safety
/// The running CPU must support the feature the kernel was compiled for;
/// [`row_kernel`] at a supported [`SimdTier`] is what establishes that.
pub(super) type RowKernel = unsafe fn(src: &[u8], dst: &[u8], out: &mut [u8], params: Blend);

/// No arch kernels: [`SimdTier`] has no value here.
#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
pub(super) fn row_kernel(tier: SimdTier, _format: ColorFormat) -> Option<RowKernel> {
    match tier {}
}

/// The SIMD row kernel compiled for `tier` itself for `format`, or `None` when
/// the tier or the format has no vector path — callers then take the scalar
/// reference.
///
/// Only RGBA is specialized: its four channels fill a vector register exactly,
/// which is what lets one register hold a pixel and the blend stay branch-free
/// across channels. Mono and RGB fall to the scalar path.
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
pub(super) fn row_kernel(tier: SimdTier, format: ColorFormat) -> Option<RowKernel> {
    #[cfg(target_arch = "aarch64")]
    use crate::ops::blend::cpu::neon as simd;
    #[cfg(target_arch = "x86_64")]
    use crate::ops::blend::cpu::sse41 as simd;

    #[cfg(target_arch = "x86_64")]
    if tier != SimdTier::Sse41 {
        return None;
    }
    #[cfg(target_arch = "aarch64")]
    let SimdTier::Neon = tier;

    if !format.has_alpha() {
        return None;
    }
    Some(match format.sample_type {
        SampleType::U8 => simd::rgba_u8_row,
        SampleType::F32 => simd::rgba_f32_row,
        SampleType::U16 => return None,
    })
}

/// Blends `src` over `dst` into `output`, all three sharing a descriptor.
pub(super) fn apply(
    params: Blend,
    src: &InterleavedImage,
    dst: &InterleavedImage,
    output: &mut InterleavedImage,
) {
    src.desc().assert_same(dst.desc(), "src/dst");
    src.desc().assert_same(output.desc(), "src/output");

    let format = src.desc().color_format();

    if let Some(kernel) = SimdTier::dispatch(|tier| row_kernel(tier, format)) {
        unsafe { apply_kernel(kernel, params, src, dst, output) };
        return;
    }

    apply_scalar(params, src, dst, output);
}

/// The scalar path, one row pair at a time through [`scalar_row`]. Split out so
/// the tests can reach the reference past the SIMD dispatch.
fn apply_scalar(
    params: Blend,
    src: &InterleavedImage,
    dst: &InterleavedImage,
    output: &mut InterleavedImage,
) {
    let format = src.desc().color_format();
    let stride = src.desc().row_byte_len();
    let (src_bytes, dst_bytes) = (src.bytes(), dst.bytes());

    par::chunks_mut(output.bytes_mut(), stride)
        .enumerate()
        .for_each(|(y, out_row)| {
            let src_row = &src_bytes[y * stride..][..stride];
            let dst_row = &dst_bytes[y * stride..][..stride];
            scalar_row(params, format, src_row, dst_row, out_row);
        });
}

/// Drives `kernel` over every row, one rayon job per row.
///
/// # Safety
/// The running CPU must support the feature `kernel` was compiled for.
unsafe fn apply_kernel(
    kernel: RowKernel,
    params: Blend,
    src: &InterleavedImage,
    dst: &InterleavedImage,
    output: &mut InterleavedImage,
) {
    let stride = src.desc().row_byte_len();
    let (src_bytes, dst_bytes) = (src.bytes(), dst.bytes());

    par::chunks_mut(output.bytes_mut(), stride)
        .enumerate()
        .for_each(|(y, out_row)| {
            let src_row = &src_bytes[y * stride..][..stride];
            let dst_row = &dst_bytes[y * stride..][..stride];
            unsafe { kernel(src_row, dst_row, out_row, params) };
        });
}

/// One channel value blended against its destination: both normalized, blended, scaled back
/// and clamped to the full scale, then narrowed by the crate's rounding rule. A NaN from a
/// float channel or alpha survives the clamp, as `f32::clamp` leaves it.
#[inline]
fn blend_value<T: Sample>(src: T, dst: T, mode: BlendMode, alpha: f32) -> T {
    let blended = mode.blend(src.to_unit(), dst.to_unit(), alpha);
    T::from_f32((blended * T::FULL_SCALE_F32).clamp(0.0, T::FULL_SCALE_F32))
}

/// Blends one pixel in place: `params.mode` over the leading `color_count` channels,
/// and — where the format carries alpha, so `color_count` is one short of the pixel —
/// a plain alpha mix over the last one, which carries no mode of its own.
#[inline]
fn blend_pixel<T: Sample>(params: Blend, src: &[T], dst: &[T], out: &mut [T], color_count: usize) {
    let Blend { mode, alpha } = params;
    for ((&s, &d), o) in src[..color_count]
        .iter()
        .zip(&dst[..color_count])
        .zip(out[..color_count].iter_mut())
    {
        *o = blend_value(s, d, mode, alpha);
    }
    for ((&s, &d), o) in src[color_count..]
        .iter()
        .zip(&dst[color_count..])
        .zip(out[color_count..].iter_mut())
    {
        *o = blend_value(s, d, BlendMode::Normal, alpha);
    }
}

/// Blends the sub-vector tail of one `RGBA` row pair through the scalar
/// reference, so a tail can never disagree with the vector body it follows.
///
/// The three slices are what [`slice::as_chunks`] left over, so each holds a
/// whole number of pixels.
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
fn rgba_tail<T: Sample>(params: Blend, src: &[T], dst: &[T], out: &mut [T]) {
    let (src, rest) = src.as_chunks::<4>();
    let (dst, _) = dst.as_chunks::<4>();
    let (out, _) = out.as_chunks_mut::<4>();
    // A partial pixel here would be dropped rather than blended, so a kernel
    // whose vector width stopped being a whole number of pixels must not pass
    // silently.
    debug_assert!(rest.is_empty(), "tail is a whole number of pixels");
    for ((src, dst), out) in src.iter().zip(dst).zip(out) {
        blend_pixel(params, src, dst, out, 3);
    }
}

/// The scalar reference over one packed row pair of `format`: per-channel
/// blending through [`blend_value`]. Taken when the CPU offers no SIMD kernel
/// for the format, and cross-checked against the SIMD kernels by the tests.
pub(super) fn scalar_row(
    params: Blend,
    format: ColorFormat,
    src: &[u8],
    dst: &[u8],
    out: &mut [u8],
) {
    match format.sample_type {
        SampleType::U8 => typed_row::<u8>(params, format, src, dst, out),
        SampleType::U16 => typed_row::<u16>(params, format, src, dst, out),
        SampleType::F32 => typed_row::<f32>(params, format, src, dst, out),
    }
}

fn typed_row<T: Sample>(
    params: Blend,
    format: ColorFormat,
    src: &[u8],
    dst: &[u8],
    out: &mut [u8],
) {
    let channel_count = format.channel_count.count();
    // Channels the blend mode applies to; alpha, where the format has one, is
    // the last channel and is left to the mix alone.
    let color_count = channel_count - usize::from(format.has_alpha());
    let src: &[T] = bytemuck::cast_slice(src);
    let dst: &[T] = bytemuck::cast_slice(dst);
    let out: &mut [T] = bytemuck::cast_slice_mut(out);

    let inputs = src
        .chunks_exact(channel_count)
        .zip(dst.chunks_exact(channel_count));
    for ((src_px, dst_px), out_px) in inputs.zip(out.chunks_exact_mut(channel_count)) {
        blend_pixel(params, src_px, dst_px, out_px, color_count);
    }
}

#[cfg(test)]
mod tests;
