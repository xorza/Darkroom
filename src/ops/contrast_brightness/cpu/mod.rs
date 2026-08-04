#[cfg(target_arch = "x86_64")]
mod avx2;
#[cfg(target_arch = "aarch64")]
mod neon;
#[cfg(target_arch = "x86_64")]
mod sse41;
#[cfg(test)]
mod tests;

use std::mem::size_of;

use bytemuck::Pod;
use rayon::prelude::*;

use crate::common::color_format::{ChannelCount, ChannelSize, ChannelType, ColorFormat};
#[cfg(target_arch = "x86_64")]
use crate::cpu_features;
use crate::image::{Image, ImageDesc};
use crate::ops::contrast_brightness::ContrastBrightness;

/// The per-channel affine `value * scale + offset`, clamped to `[0, max]`, that
/// contrast/brightness reduces to in a storage type's own units.
///
/// Folding brightness and the mid-point recentering into one offset is what
/// lets a row be a single multiply-add. The scalar reference and every SIMD
/// kernel evaluate exactly this expression in exactly this order, so their
/// integer results agree bit for bit rather than within a rounding tolerance.
#[derive(Debug, Clone, Copy)]
pub(super) struct ChannelAffine {
    scale: f32,
    offset: f32,
    max: f32,
}

impl ChannelAffine {
    fn new(params: &ContrastBrightness, format: ColorFormat) -> Self {
        let max = match (format.channel_size, format.channel_type) {
            (ChannelSize::_8bit, ChannelType::UInt) => f32::from(u8::MAX),
            (ChannelSize::_16bit, ChannelType::UInt) => f32::from(u16::MAX),
            (ChannelSize::_32bit, ChannelType::Float) => 1.0,
            _ => unreachable!("unsupported color format for contrast/brightness"),
        };
        let mid = max / 2.0;
        Self {
            scale: params.contrast,
            offset: mid * (1.0 - params.contrast) + params.brightness * max,
            max,
        }
    }
}

/// A SIMD row kernel applying [`ChannelAffine`] to a row in place, over `count`
/// items: channel values for the flat kernels, whole pixels for the
/// alpha-preserving ones.
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
type RowKernel = unsafe fn(&mut [u8], usize, ChannelAffine);

/// The SIMD row kernel for `format` on this arch (SSE4.1 / NEON), or `None`
/// when the CPU lacks the feature — callers then fall back to the scalar path.
///
/// `L` and `RGB` have no channel to protect, so they take a flat kernel that
/// walks the row as one contiguous channel array and never pays for pixel
/// boundaries; only `RGBA` needs the per-pixel form that carries alpha through.
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
fn row_kernel(format: ColorFormat) -> Option<RowKernel> {
    #[cfg(target_arch = "x86_64")]
    let kernel = if cpu_features::has_avx2() {
        avx2_kernel(format)?
    } else if cpu_features::has_sse4_1() {
        sse41_kernel(format)?
    } else {
        return None;
    };

    #[cfg(target_arch = "aarch64")]
    let kernel = neon_kernel(format)?;

    Some(kernel)
}

/// Splits `format` into the shape the kernel tables key on: storage type, and
/// whether there is an alpha channel to carry through.
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
fn kernel_key(format: ColorFormat) -> (ChannelSize, ChannelType, bool) {
    (
        format.channel_size,
        format.channel_type,
        format.channel_count == ChannelCount::Rgba,
    )
}

/// SAFETY: the returned kernels require AVX2; callers must have verified it.
#[cfg(target_arch = "x86_64")]
fn avx2_kernel(format: ColorFormat) -> Option<RowKernel> {
    Some(match kernel_key(format) {
        (ChannelSize::_8bit, ChannelType::UInt, false) => avx2::u8_flat as RowKernel,
        (ChannelSize::_8bit, ChannelType::UInt, true) => avx2::u8_rgba as RowKernel,
        (ChannelSize::_16bit, ChannelType::UInt, false) => avx2::u16_flat as RowKernel,
        (ChannelSize::_16bit, ChannelType::UInt, true) => avx2::u16_rgba as RowKernel,
        (ChannelSize::_32bit, ChannelType::Float, false) => avx2::f32_flat as RowKernel,
        (ChannelSize::_32bit, ChannelType::Float, true) => avx2::f32_rgba as RowKernel,
        _ => return None,
    })
}

/// SAFETY: the returned kernels require SSE4.1; callers must have verified it.
#[cfg(target_arch = "x86_64")]
fn sse41_kernel(format: ColorFormat) -> Option<RowKernel> {
    Some(match kernel_key(format) {
        (ChannelSize::_8bit, ChannelType::UInt, false) => sse41::u8_flat as RowKernel,
        (ChannelSize::_8bit, ChannelType::UInt, true) => sse41::u8_rgba as RowKernel,
        (ChannelSize::_16bit, ChannelType::UInt, false) => sse41::u16_flat as RowKernel,
        (ChannelSize::_16bit, ChannelType::UInt, true) => sse41::u16_rgba as RowKernel,
        (ChannelSize::_32bit, ChannelType::Float, false) => sse41::f32_flat as RowKernel,
        (ChannelSize::_32bit, ChannelType::Float, true) => sse41::f32_rgba as RowKernel,
        _ => return None,
    })
}

/// SAFETY: NEON is baseline on aarch64, so these are always callable.
#[cfg(target_arch = "aarch64")]
fn neon_kernel(format: ColorFormat) -> Option<RowKernel> {
    Some(match kernel_key(format) {
        (ChannelSize::_8bit, ChannelType::UInt, false) => neon::u8_flat as RowKernel,
        (ChannelSize::_8bit, ChannelType::UInt, true) => neon::u8_rgba as RowKernel,
        (ChannelSize::_16bit, ChannelType::UInt, false) => neon::u16_flat as RowKernel,
        (ChannelSize::_16bit, ChannelType::UInt, true) => neon::u16_rgba as RowKernel,
        (ChannelSize::_32bit, ChannelType::Float, false) => neon::f32_flat as RowKernel,
        (ChannelSize::_32bit, ChannelType::Float, true) => neon::f32_rgba as RowKernel,
        _ => return None,
    })
}

/// Applies contrast and brightness adjustment to an image in place using CPU.
/// The kernels read and write the same row, so the path holds no scratch buffer
/// and allocates nothing.
///
/// One rayon job per row. Rows carry no padding, so coarser jobs would be
/// correct too, but they measure slower: on a hybrid core CPU the small jobs are
/// what lets work-stealing keep the efficiency cores from holding up a frame.
pub(super) fn apply(params: &ContrastBrightness, image: &mut Image) {
    let format = image.desc().color_format;

    #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
    if let Some(kernel) = row_kernel(format) {
        // SAFETY: `row_kernel` verified this CPU has the kernel's feature.
        unsafe { apply_kernel(kernel, params, image) };
        return;
    }

    match (format.channel_size, format.channel_type) {
        (ChannelSize::_8bit, ChannelType::UInt) => {
            apply_typed::<u8>(image, *params);
        }
        (ChannelSize::_16bit, ChannelType::UInt) => {
            apply_typed::<u16>(image, *params);
        }
        (ChannelSize::_32bit, ChannelType::Float) => {
            apply_typed::<f32>(image, *params);
        }
        _ => {
            unreachable!("Unsupported color format for contrast/brightness")
        }
    }
}

/// How many items a kernel is asked to walk per row: channel values for the
/// flat kernels, whole pixels for the alpha-preserving ones.
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
fn kernel_count(desc: ImageDesc) -> usize {
    match desc.color_format.channel_count {
        ChannelCount::Rgba => desc.width,
        channels => desc.width * channels.channel_count() as usize,
    }
}

/// Drives `kernel` over every row of `image`, one rayon job per row.
///
/// # Safety
/// The running CPU must support the feature `kernel` was compiled for.
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
unsafe fn apply_kernel(kernel: RowKernel, params: &ContrastBrightness, image: &mut Image) {
    let format = image.desc().color_format;
    let affine = ChannelAffine::new(params, format);
    let count = kernel_count(image.desc());
    let stride = image.desc().row_bytes();

    image.bytes_mut().par_chunks_mut(stride).for_each(|row| {
        // SAFETY: forwarded from this function's own contract.
        unsafe { kernel(row, count, affine) };
    });
}

pub(super) trait ContrastBrightnessApply: Pod + Send + Sync {
    fn apply(self, affine: ChannelAffine) -> Self;
}

impl ContrastBrightnessApply for u8 {
    #[inline]
    fn apply(self, affine: ChannelAffine) -> Self {
        (f32::from(self) * affine.scale + affine.offset)
            .clamp(0.0, affine.max)
            .round_ties_even() as Self
    }
}

impl ContrastBrightnessApply for u16 {
    #[inline]
    fn apply(self, affine: ChannelAffine) -> Self {
        (f32::from(self) * affine.scale + affine.offset)
            .clamp(0.0, affine.max)
            .round_ties_even() as Self
    }
}

impl ContrastBrightnessApply for f32 {
    #[inline]
    fn apply(self, affine: ChannelAffine) -> Self {
        (self * affine.scale + affine.offset).clamp(0.0, affine.max)
    }
}

/// The scalar reference: per-element in-place adjustment through
/// [`ContrastBrightnessApply`]. Taken when the CPU offers no SIMD kernel for
/// the format, and cross-checked against the SIMD kernels by the tests.
pub(super) fn apply_typed<T>(image: &mut Image, params: ContrastBrightness)
where
    T: Pod + ContrastBrightnessApply,
{
    let format = image.desc().color_format;
    debug_assert_eq!(
        format.channel_size.byte_count() as usize,
        size_of::<T>(),
        "storage type does not match the image's channel size"
    );

    let affine = ChannelAffine::new(&params, format);
    let width = image.desc().width;
    let channels = format.channel_count.channel_count() as usize;
    let stride = image.desc().row_bytes();
    let row_bytes = width * channels * size_of::<T>();
    let has_alpha = format.channel_count == ChannelCount::Rgba;

    image.bytes_mut().par_chunks_mut(stride).for_each(|row| {
        let row: &mut [T] = bytemuck::cast_slice_mut(&mut row[..row_bytes]);
        if has_alpha {
            for pixel in row.chunks_exact_mut(channels) {
                // Alpha, the last channel, is left untouched.
                for value in &mut pixel[..channels - 1] {
                    *value = value.apply(affine);
                }
            }
        } else {
            // Nothing to protect, so the row is one flat channel array.
            for value in row.iter_mut() {
                *value = value.apply(affine);
            }
        }
    });
}
