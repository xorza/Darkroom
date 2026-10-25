#[cfg(target_arch = "x86_64")]
mod avx2;
#[cfg(target_arch = "aarch64")]
mod neon;
#[cfg(target_arch = "x86_64")]
mod sse41;

use common::par;
use rayon::prelude::*;

use crate::color::color_format::{ColorFormat, SampleType};
use crate::color::sample::Sample;
use crate::image::Image;
use crate::image::image_desc::ImageDesc;
use crate::ops::contrast_brightness::ContrastBrightness;
use crate::ops::contrast_brightness::channel_affine::ChannelAffine;
use crate::simd_tier::SimdTier;

/// A SIMD row kernel applying [`ChannelAffine`] to a row in place, over `count`
/// items: channel values for the flat kernels, whole pixels for the
/// alpha-preserving ones.
pub(super) type RowKernel = unsafe fn(&mut [u8], usize, ChannelAffine);

/// No arch kernels: [`SimdTier`] has no value here.
#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
pub(super) fn row_kernel(tier: SimdTier, _format: ColorFormat) -> Option<RowKernel> {
    match tier {}
}

/// The SIMD row kernel compiled for `tier` itself for `format`, or `None` when
/// that tier has none. The FMA tier has none: dispatch walks down to the AVX2
/// kernels, which fuse nothing, and below SSE4.1 to the scalar path.
///
/// `L` and `RGB` have no channel to protect, so they take a flat kernel that
/// walks the row as one contiguous channel array and never pays for pixel
/// boundaries; only `RGBA` needs the per-pixel form that carries alpha through.
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
#[cfg_attr(
    target_arch = "aarch64",
    expect(
        clippy::unnecessary_wraps,
        reason = "NEON is baseline; it is x86_64 that has tiers without the kernels"
    )
)]
pub(super) fn row_kernel(tier: SimdTier, format: ColorFormat) -> Option<RowKernel> {
    #[cfg(target_arch = "x86_64")]
    let kernel = match tier {
        SimdTier::Avx2 => avx2_kernel(format),
        SimdTier::Sse41 => sse41_kernel(format),
        SimdTier::Sse2 | SimdTier::Ssse3 | SimdTier::Avx2Fma => return None,
    };

    #[cfg(target_arch = "aarch64")]
    let SimdTier::Neon = tier;
    #[cfg(target_arch = "aarch64")]
    let kernel = neon_kernel(format);

    Some(kernel)
}

/// The kernels need AVX2, which the caller verified.
#[cfg(target_arch = "x86_64")]
fn avx2_kernel(format: ColorFormat) -> RowKernel {
    match (format.sample_type, format.has_alpha()) {
        (SampleType::U8, false) => avx2::u8_flat,
        (SampleType::U8, true) => avx2::u8_rgba,
        (SampleType::U16, false) => avx2::u16_flat,
        (SampleType::U16, true) => avx2::u16_rgba,
        (SampleType::F32, false) => avx2::f32_flat,
        (SampleType::F32, true) => avx2::f32_rgba,
    }
}

/// The kernels need SSE4.1, which the caller verified.
#[cfg(target_arch = "x86_64")]
fn sse41_kernel(format: ColorFormat) -> RowKernel {
    match (format.sample_type, format.has_alpha()) {
        (SampleType::U8, false) => sse41::u8_flat,
        (SampleType::U8, true) => sse41::u8_rgba,
        (SampleType::U16, false) => sse41::u16_flat,
        (SampleType::U16, true) => sse41::u16_rgba,
        (SampleType::F32, false) => sse41::f32_flat,
        (SampleType::F32, true) => sse41::f32_rgba,
    }
}

/// The kernels need NEON, which is baseline on aarch64.
#[cfg(target_arch = "aarch64")]
fn neon_kernel(format: ColorFormat) -> RowKernel {
    match (format.sample_type, format.has_alpha()) {
        (SampleType::U8, false) => neon::u8_flat,
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

    if let Some(kernel) = SimdTier::dispatch(|tier| row_kernel(tier, format)) {
        unsafe { apply_kernel(kernel, params, image) };
        return;
    }

    apply_scalar(params, image);
}

/// The scalar path, one row at a time through [`scalar_row`]. Split out so the
/// tests can reach the reference past the SIMD dispatch.
pub(super) fn apply_scalar(params: ContrastBrightness, image: &mut Image) {
    let format = image.desc().color_format;
    let affine = ChannelAffine::new(params, format);
    let stride = image.desc().row_bytes();

    par::chunks_mut(image.bytes_mut(), stride).for_each(|row| scalar_row(affine, format, row));
}

/// The scalar reference over one packed row of `format` in place, through
/// [`ChannelAffine::apply`]. Taken when the CPU offers no SIMD kernel for the
/// format, and cross-checked against the SIMD kernels by the tests.
pub(super) fn scalar_row(affine: ChannelAffine, format: ColorFormat, row: &mut [u8]) {
    match format.sample_type {
        SampleType::U8 => typed_row::<u8>(affine, format, row),
        SampleType::U16 => typed_row::<u16>(affine, format, row),
        SampleType::F32 => typed_row::<f32>(affine, format, row),
    }
}

fn typed_row<T: Sample>(affine: ChannelAffine, format: ColorFormat, row: &mut [u8]) {
    let row: &mut [T] = bytemuck::cast_slice_mut(row);
    if format.has_alpha() {
        affine.apply_rgba(row);
    } else {
        affine.apply_all(row);
    }
}

/// How many items a kernel is asked to walk per row: channel values for the
/// flat kernels, whole pixels for the alpha-preserving ones.
pub(super) const fn kernel_count(desc: ImageDesc) -> usize {
    if desc.color_format.has_alpha() {
        desc.size.width
    } else {
        desc.size.width * desc.color_format.channel_count.count()
    }
}

/// Drives `kernel` over every row of `image`, one rayon job per row.
///
/// # Safety
/// The running CPU must support the feature `kernel` was compiled for.
unsafe fn apply_kernel(kernel: RowKernel, params: ContrastBrightness, image: &mut Image) {
    let format = image.desc().color_format;
    let affine = ChannelAffine::new(params, format);
    let count = kernel_count(image.desc());
    let stride = image.desc().row_bytes();

    par::chunks_mut(image.bytes_mut(), stride).for_each(|row| {
        unsafe { kernel(row, count, affine) };
    });
}

#[cfg(test)]
mod tests;
