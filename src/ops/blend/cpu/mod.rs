#[cfg(target_arch = "aarch64")]
mod neon;
#[cfg(target_arch = "x86_64")]
mod sse41;

use rayon::prelude::*;

use crate::common::color_format::{ColorFormat, SampleType};
use crate::common::sample::Sample;
use crate::image::Image;
use crate::ops::blend::{Blend, BlendMode};
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
use crate::simd_tier::SimdTier;

/// A SIMD row kernel blending one `src`/`dst` row pair into `out`. All three are
/// exactly one packed row, so a kernel's pixel count is its slice length and it
/// never has cause to read past the row it was given.
///
/// # Safety
/// The running CPU must support the feature the kernel was compiled for;
/// [`row_kernel`] at a supported [`SimdTier`] is what establishes that.
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
type RowKernel = unsafe fn(src: &[u8], dst: &[u8], out: &mut [u8], params: Blend);

/// The SIMD row kernel `tier` has for `format`, or `None` when the tier or the
/// format has no vector path — callers then take the scalar reference.
///
/// Only RGBA is specialized: its four channels fill a vector register exactly,
/// which is what lets one register hold a pixel and the blend stay branch-free
/// across channels. L and RGB fall to the scalar path.
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
fn row_kernel(tier: SimdTier, format: ColorFormat) -> Option<RowKernel> {
    #[cfg(target_arch = "aarch64")]
    use crate::ops::blend::cpu::neon as simd;
    #[cfg(target_arch = "x86_64")]
    use crate::ops::blend::cpu::sse41 as simd;

    #[cfg(target_arch = "x86_64")]
    if tier < SimdTier::Sse41 {
        return None;
    }
    #[cfg(target_arch = "aarch64")]
    let SimdTier::Neon = tier;

    if !format.has_alpha() {
        return None;
    }
    Some(match format.sample_type {
        SampleType::U8 => simd::rgba_u8_row as RowKernel,
        SampleType::F32 => simd::rgba_f32_row as RowKernel,
        SampleType::U16 => return None,
    })
}

/// Blends `src` over `dst` into `output`, all three sharing a descriptor.
pub(super) fn apply(params: Blend, src: &Image, dst: &Image, output: &mut Image) {
    src.desc().assert_same(dst.desc(), "src/dst");
    src.desc().assert_same(output.desc(), "src/output");

    let format = src.desc().color_format;

    #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
    if let Some(kernel) = SimdTier::widest().and_then(|tier| row_kernel(tier, format)) {
        // SAFETY: the kernel is the widest supported tier's.
        unsafe { apply_kernel(kernel, params, src, dst, output) };
        return;
    }

    apply_scalar(params, src, dst, output);
}

/// The scalar path, picking the storage type the format stores channels in.
/// Split out so the tests can reach the reference past the SIMD dispatch.
fn apply_scalar(params: Blend, src: &Image, dst: &Image, output: &mut Image) {
    match src.desc().color_format.sample_type {
        SampleType::U8 => apply_typed::<u8>(params, src, dst, output),
        SampleType::U16 => apply_typed::<u16>(params, src, dst, output),
        SampleType::F32 => apply_typed::<f32>(params, src, dst, output),
    }
}

/// Drives `kernel` over every row, one rayon job per row.
///
/// # Safety
/// The running CPU must support the feature `kernel` was compiled for.
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
unsafe fn apply_kernel(
    kernel: RowKernel,
    params: Blend,
    src: &Image,
    dst: &Image,
    output: &mut Image,
) {
    let stride = src.desc().row_bytes();
    let (src_bytes, dst_bytes) = (src.bytes(), dst.bytes());

    output
        .bytes_mut()
        .par_chunks_mut(stride)
        .enumerate()
        .for_each(|(y, out_row)| {
            let src_row = &src_bytes[y * stride..][..stride];
            let dst_row = &dst_bytes[y * stride..][..stride];
            // SAFETY: forwarded from this function's own contract.
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

/// Blends one pixel in place: `params.mode` over the leading `color` channels,
/// and — where the format carries alpha, so `color` is one short of the pixel —
/// a plain alpha mix over the last one, which carries no mode of its own.
#[inline]
fn blend_pixel<T: Sample>(params: Blend, src: &[T], dst: &[T], out: &mut [T], color: usize) {
    let Blend { mode, alpha } = params;
    for ((&s, &d), o) in src[..color]
        .iter()
        .zip(&dst[..color])
        .zip(out[..color].iter_mut())
    {
        *o = blend_value(s, d, mode, alpha);
    }
    for ((&s, &d), o) in src[color..]
        .iter()
        .zip(&dst[color..])
        .zip(out[color..].iter_mut())
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

/// The scalar reference: per-channel blending through [`blend_value`]. Taken when
/// the CPU offers no SIMD kernel for the format, and cross-checked against the
/// SIMD kernels by the tests.
fn apply_typed<T>(params: Blend, src: &Image, dst: &Image, output: &mut Image)
where
    T: Sample,
{
    let format = src.desc().color_format;
    debug_assert_eq!(format.sample_type, T::TYPE);

    let channels = format.channel_count.count();
    let stride = src.desc().row_bytes();
    // Channels the blend mode applies to; alpha, where the format has one, is
    // the last channel and is left to the mix alone.
    let color = channels - usize::from(format.has_alpha());
    let (src_bytes, dst_bytes) = (src.bytes(), dst.bytes());

    output
        .bytes_mut()
        .par_chunks_mut(stride)
        .enumerate()
        .for_each(|(y, out_row)| {
            let src_row: &[T] = bytemuck::cast_slice(&src_bytes[y * stride..][..stride]);
            let dst_row: &[T] = bytemuck::cast_slice(&dst_bytes[y * stride..][..stride]);
            let out_row: &mut [T] = bytemuck::cast_slice_mut(out_row);

            let inputs = src_row
                .chunks_exact(channels)
                .zip(dst_row.chunks_exact(channels));
            for ((src_px, dst_px), out_px) in inputs.zip(out_row.chunks_exact_mut(channels)) {
                blend_pixel(params, src_px, dst_px, out_px, color);
            }
        });
}

#[cfg(test)]
mod tests;
