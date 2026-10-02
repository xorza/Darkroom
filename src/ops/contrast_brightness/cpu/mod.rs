#[cfg(target_arch = "x86_64")]
mod avx2;
#[cfg(target_arch = "aarch64")]
mod neon;
#[cfg(target_arch = "x86_64")]
mod sse41;

use rayon::prelude::*;

use crate::common::color_format::{ColorFormat, SampleType};
use crate::common::sample::Sample;
#[cfg(target_arch = "x86_64")]
use crate::cpu_features;
use crate::image::Image;
use crate::image::image_desc::ImageDesc;
use crate::ops::contrast_brightness::ContrastBrightness;
use crate::ops::contrast_brightness::channel_affine::ChannelAffine;

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
#[cfg_attr(
    target_arch = "aarch64",
    expect(
        clippy::unnecessary_wraps,
        reason = "NEON is baseline; it is x86_64 that has CPUs without the kernels' features"
    )
)]
fn row_kernel(format: ColorFormat) -> Option<RowKernel> {
    #[cfg(target_arch = "x86_64")]
    let kernel = if cpu_features::has_avx2() {
        avx2_kernel(format)
    } else if cpu_features::has_sse4_1() {
        sse41_kernel(format)
    } else {
        return None;
    };

    #[cfg(target_arch = "aarch64")]
    let kernel = neon_kernel(format);

    Some(kernel)
}

/// SAFETY: the returned kernels require AVX2; callers must have verified it.
#[cfg(target_arch = "x86_64")]
fn avx2_kernel(format: ColorFormat) -> RowKernel {
    match (format.sample_type, format.has_alpha()) {
        (SampleType::U8, false) => avx2::u8_flat as RowKernel,
        (SampleType::U8, true) => avx2::u8_rgba,
        (SampleType::U16, false) => avx2::u16_flat,
        (SampleType::U16, true) => avx2::u16_rgba,
        (SampleType::F32, false) => avx2::f32_flat,
        (SampleType::F32, true) => avx2::f32_rgba,
    }
}

/// SAFETY: the returned kernels require SSE4.1; callers must have verified it.
#[cfg(target_arch = "x86_64")]
fn sse41_kernel(format: ColorFormat) -> RowKernel {
    match (format.sample_type, format.has_alpha()) {
        (SampleType::U8, false) => sse41::u8_flat as RowKernel,
        (SampleType::U8, true) => sse41::u8_rgba,
        (SampleType::U16, false) => sse41::u16_flat,
        (SampleType::U16, true) => sse41::u16_rgba,
        (SampleType::F32, false) => sse41::f32_flat,
        (SampleType::F32, true) => sse41::f32_rgba,
    }
}

/// SAFETY: NEON is baseline on aarch64, so these are always callable.
#[cfg(target_arch = "aarch64")]
fn neon_kernel(format: ColorFormat) -> RowKernel {
    match (format.sample_type, format.has_alpha()) {
        (SampleType::U8, false) => neon::u8_flat as RowKernel,
        (SampleType::U8, true) => neon::u8_rgba,
        (SampleType::U16, false) => neon::u16_flat,
        (SampleType::U16, true) => neon::u16_rgba,
        (SampleType::F32, false) => neon::f32_flat,
        (SampleType::F32, true) => neon::f32_rgba,
    }
}

/// Applies contrast and brightness adjustment to an image in place using CPU.
/// The kernels read and write the same row, so the path holds no scratch buffer
/// and allocates nothing.
///
/// One rayon job per row. Rows carry no padding, so coarser jobs would be
/// correct too, but they measure slower: on a hybrid core CPU the small jobs are
/// what lets work-stealing keep the efficiency cores from holding up a frame.
pub(super) fn apply(params: ContrastBrightness, image: &mut Image) {
    let format = image.desc().color_format;

    #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
    if let Some(kernel) = row_kernel(format) {
        // SAFETY: `row_kernel` verified this CPU has the kernel's feature.
        unsafe { apply_kernel(kernel, params, image) };
        return;
    }

    apply_scalar(params, image);
}

/// The scalar path, picking the storage type the format stores channels in.
/// Split out so the tests and benches can reach the reference past the SIMD
/// dispatch.
pub(super) fn apply_scalar(params: ContrastBrightness, image: &mut Image) {
    match image.desc().color_format.sample_type {
        SampleType::U8 => apply_typed::<u8>(params, image),
        SampleType::U16 => apply_typed::<u16>(params, image),
        SampleType::F32 => apply_typed::<f32>(params, image),
    }
}

/// How many items a kernel is asked to walk per row: channel values for the
/// flat kernels, whole pixels for the alpha-preserving ones.
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
fn kernel_count(desc: ImageDesc) -> usize {
    if desc.color_format.has_alpha() {
        desc.width
    } else {
        desc.width * desc.color_format.channel_count.count()
    }
}

/// Drives `kernel` over every row of `image`, one rayon job per row.
///
/// # Safety
/// The running CPU must support the feature `kernel` was compiled for.
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
unsafe fn apply_kernel(kernel: RowKernel, params: ContrastBrightness, image: &mut Image) {
    let format = image.desc().color_format;
    let affine = ChannelAffine::new(params, format);
    let count = kernel_count(image.desc());
    let stride = image.desc().row_bytes();

    image.bytes_mut().par_chunks_mut(stride).for_each(|row| {
        // SAFETY: forwarded from this function's own contract.
        unsafe { kernel(row, count, affine) };
    });
}

/// The scalar reference: per-element in-place adjustment through
/// [`ChannelAffine::apply`]. Taken when the CPU offers no SIMD kernel for
/// the format, and cross-checked against the SIMD kernels by the tests.
pub(super) fn apply_typed<T>(params: ContrastBrightness, image: &mut Image)
where
    T: Sample,
{
    let format = image.desc().color_format;
    debug_assert_eq!(format.sample_type, T::TYPE);

    let affine = ChannelAffine::new(params, format);
    let channels = format.channel_count.count();
    let stride = image.desc().row_bytes();
    let has_alpha = format.has_alpha();

    image.bytes_mut().par_chunks_mut(stride).for_each(|row| {
        let row: &mut [T] = bytemuck::cast_slice_mut(row);
        if has_alpha {
            for pixel in row.chunks_exact_mut(channels) {
                // Alpha, the last channel, is left untouched.
                for value in &mut pixel[..channels - 1] {
                    *value = affine.apply(*value);
                }
            }
        } else {
            // Nothing to protect, so the row is one flat channel array.
            for value in row.iter_mut() {
                *value = affine.apply(*value);
            }
        }
    });
}

#[cfg(test)]
mod tests;
