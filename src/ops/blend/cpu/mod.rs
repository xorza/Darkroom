#[cfg(target_arch = "aarch64")]
mod neon;
#[cfg(target_arch = "x86_64")]
mod sse41;
#[cfg(test)]
mod tests;

use bytemuck::Pod;
use rayon::prelude::*;

use crate::common::color_format::{ChannelCount, ChannelSize, ChannelType, ColorFormat};
#[cfg(target_arch = "x86_64")]
use crate::cpu_features;
use crate::image::Image;
use crate::ops::blend::{Blend, BlendMode};

/// A SIMD row kernel blending one `src`/`dst` row pair into `out`. All three are
/// exactly one packed row, so a kernel's pixel count is its slice length and it
/// never has cause to read past the row it was given.
///
/// # Safety
/// The running CPU must support the feature the kernel was compiled for;
/// [`row_kernel`] is what establishes that.
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
type RowKernel = unsafe fn(src: &[u8], dst: &[u8], out: &mut [u8], params: Blend);

/// The SIMD row kernel for `format` on this arch, or `None` when the CPU lacks
/// the feature or the format has no vector path — callers then take the scalar
/// reference.
///
/// Only RGBA is specialized: its four channels fill a vector register exactly,
/// which is what lets one register hold a pixel and the blend stay branch-free
/// across channels. L and RGB fall to the scalar path.
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
fn row_kernel(format: ColorFormat) -> Option<RowKernel> {
    #[cfg(target_arch = "aarch64")]
    use crate::ops::blend::cpu::neon as simd;
    #[cfg(target_arch = "x86_64")]
    use crate::ops::blend::cpu::sse41 as simd;

    #[cfg(target_arch = "x86_64")]
    if !cpu_features::has_sse4_1() {
        return None;
    }

    if format.channel_count != ChannelCount::Rgba {
        return None;
    }
    Some(match (format.channel_size, format.channel_type) {
        (ChannelSize::_8bit, ChannelType::UInt) => simd::rgba_u8_row as RowKernel,
        (ChannelSize::_32bit, ChannelType::Float) => simd::rgba_f32_row as RowKernel,
        _ => return None,
    })
}

/// Blends `src` over `dst` into `output`, all three sharing a descriptor.
pub(super) fn apply(params: &Blend, src: &Image, dst: &Image, output: &mut Image) {
    src.desc().assert_same(dst.desc(), "src/dst");
    src.desc().assert_same(output.desc(), "src/output");

    let format = src.desc().color_format;

    #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
    if let Some(kernel) = row_kernel(format) {
        // SAFETY: `row_kernel` verified this CPU has the kernel's feature.
        unsafe { apply_kernel(kernel, *params, src, dst, output) };
        return;
    }

    apply_scalar(*params, src, dst, output);
}

/// The scalar path, picking the storage type the format stores channels in.
/// Split out so the tests can reach the reference past the SIMD dispatch.
fn apply_scalar(params: Blend, src: &Image, dst: &Image, output: &mut Image) {
    let format = src.desc().color_format;
    match (format.channel_size, format.channel_type) {
        (ChannelSize::_8bit, ChannelType::UInt) => apply_typed::<u8>(params, src, dst, output),
        (ChannelSize::_16bit, ChannelType::UInt) => apply_typed::<u16>(params, src, dst, output),
        (ChannelSize::_32bit, ChannelType::Float) => apply_typed::<f32>(params, src, dst, output),
        _ => unreachable!("unsupported color format for blend: {format:?}"),
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

/// One channel value in a storage type, blended against its destination.
trait BlendApply: Pod + Send + Sync {
    fn blend(self, dst: Self, mode: BlendMode, alpha: f32) -> Self;
}

impl BlendApply for u8 {
    #[inline]
    fn blend(self, dst: Self, mode: BlendMode, alpha: f32) -> Self {
        let max = f32::from(Self::MAX);
        let result = mode.blend(f32::from(self) / max, f32::from(dst) / max, alpha);
        (result * max).clamp(0.0, max) as Self
    }
}

impl BlendApply for u16 {
    #[inline]
    fn blend(self, dst: Self, mode: BlendMode, alpha: f32) -> Self {
        let max = f32::from(Self::MAX);
        let result = mode.blend(f32::from(self) / max, f32::from(dst) / max, alpha);
        (result * max).clamp(0.0, max) as Self
    }
}

impl BlendApply for f32 {
    #[inline]
    fn blend(self, dst: Self, mode: BlendMode, alpha: f32) -> Self {
        mode.blend(self, dst, alpha).clamp(0.0, 1.0)
    }
}

/// Blends one pixel in place: `params.mode` over the leading `color` channels,
/// and — where the format carries alpha, so `color` is one short of the pixel —
/// a plain alpha mix over the last one, which carries no mode of its own.
#[inline]
fn blend_pixel<T: BlendApply>(params: Blend, src: &[T], dst: &[T], out: &mut [T], color: usize) {
    let Blend { mode, alpha } = params;
    for ((&s, &d), o) in src[..color]
        .iter()
        .zip(&dst[..color])
        .zip(out[..color].iter_mut())
    {
        *o = s.blend(d, mode, alpha);
    }
    for ((&s, &d), o) in src[color..]
        .iter()
        .zip(&dst[color..])
        .zip(out[color..].iter_mut())
    {
        *o = s.blend(d, BlendMode::Normal, alpha);
    }
}

/// Blends the sub-vector tail of one `RGBA` row pair through the scalar
/// reference, so a tail can never disagree with the vector body it follows.
///
/// The three slices are what [`slice::as_chunks`] left over, so each holds a
/// whole number of pixels.
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
fn rgba_tail<T: BlendApply>(params: Blend, src: &[T], dst: &[T], out: &mut [T]) {
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

/// The scalar reference: per-channel blending through [`BlendApply`]. Taken when
/// the CPU offers no SIMD kernel for the format, and cross-checked against the
/// SIMD kernels by the tests.
fn apply_typed<T>(params: Blend, src: &Image, dst: &Image, output: &mut Image)
where
    T: BlendApply,
{
    let format = src.desc().color_format;
    debug_assert_eq!(
        format.channel_size.byte_count() as usize,
        size_of::<T>(),
        "storage type does not match the image's channel size"
    );

    let channels = format.channel_count.channel_count() as usize;
    let stride = src.desc().row_bytes();
    // Channels the blend mode applies to; alpha, where the format has one, is
    // the last channel and is left to the mix alone.
    let color = channels - usize::from(format.channel_count == ChannelCount::Rgba);
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
