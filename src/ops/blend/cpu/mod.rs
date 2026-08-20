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

/// A SIMD row kernel blending one `src`/`dst` row pair of `width` pixels into
/// `out`. The three slices start at the same pixel and run to the end of their
/// images, so a kernel may read past `width` pixels only as far as its own
/// vector width allows.
///
/// # Safety
/// The running CPU must support the feature the kernel was compiled for;
/// [`row_kernel`] is what establishes that.
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
type RowKernel = unsafe fn(src: &[u8], dst: &[u8], out: &mut [u8], width: usize, params: Blend);

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
    let width = src.desc().width;
    let stride = src.desc().row_bytes();
    let (src_bytes, dst_bytes) = (src.bytes(), dst.bytes());

    output
        .bytes_mut()
        .par_chunks_mut(stride)
        .enumerate()
        .for_each(|(y, out_row)| {
            // SAFETY: forwarded from this function's own contract.
            unsafe {
                kernel(
                    &src_bytes[y * stride..],
                    &dst_bytes[y * stride..],
                    out_row,
                    width,
                    params,
                )
            };
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
    let has_alpha = format.channel_count == ChannelCount::Rgba;
    // Channels the blend mode applies to; an alpha channel, when present, is the
    // last one and carries no mode of its own — only the alpha mix.
    let color = if has_alpha { channels - 1 } else { channels };
    let Blend { mode, alpha } = params;
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
                for ((&s, &d), out) in src_px[..color]
                    .iter()
                    .zip(&dst_px[..color])
                    .zip(out_px[..color].iter_mut())
                {
                    *out = s.blend(d, mode, alpha);
                }
                if has_alpha {
                    out_px[color] = src_px[color].blend(dst_px[color], BlendMode::Normal, alpha);
                }
            }
        });
}
