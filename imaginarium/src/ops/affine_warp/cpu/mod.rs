/// NEON RGB/RGBA bilinear specialization of the scalar path below (aarch64 only).
#[cfg(target_arch = "aarch64")]
mod neon;

/// SSE4.1 RGB/RGBA bilinear specialization of the scalar path below (x86_64).
#[cfg(target_arch = "x86_64")]
mod sse;

mod packed_source;

use common::{SimdTier, par};
use glam::Vec2;
use rayon::prelude::*;

use crate::color::color_format::{ChannelCount, ColorFormat, SampleType};
use crate::color::sample::Sample;
use crate::image::InterleavedImage;
use crate::ops::affine_warp::cpu::packed_source::PackedSource;
use crate::ops::affine_warp::{AffineWarp, FilterMode};

/// A SIMD kernel: the packed RGB/RGBA bilinear specialization of [`apply_typed`]
/// for one storage type and channel count.
///
/// # Safety
/// The running CPU must support the feature the kernel was compiled for;
/// [`packed_kernel`] at a supported [`SimdTier`] is what establishes that.
pub(super) type PackedKernel = unsafe fn(&AffineWarp, &InterleavedImage, &mut InterleavedImage);

/// No arch kernels: [`SimdTier`] has no value here.
#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
pub(super) fn packed_kernel(
    tier: SimdTier,
    _format: ColorFormat,
    _filter: FilterMode,
) -> Option<PackedKernel> {
    match tier {}
}

/// The SIMD kernel compiled for `tier` itself for `format` under `filter`, or
/// `None` when the tier or the combination has no vector path — callers then
/// take the scalar reference.
///
/// RGB/RGBA bilinear vectorize and are bit-identical to the scalar reference
/// (cross-checked). Mono stays scalar (gather-bound — SIMD measured slower), and
/// nearest is a near-memcpy the scalar path already nails.
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
pub(super) fn packed_kernel(
    tier: SimdTier,
    format: ColorFormat,
    filter: FilterMode,
) -> Option<PackedKernel> {
    #[cfg(target_arch = "aarch64")]
    use crate::ops::affine_warp::cpu::neon as simd;
    #[cfg(target_arch = "x86_64")]
    use crate::ops::affine_warp::cpu::sse as simd;

    if filter != FilterMode::Bilinear {
        return None;
    }

    #[cfg(target_arch = "x86_64")]
    if tier != SimdTier::Sse41 {
        return None;
    }
    #[cfg(target_arch = "aarch64")]
    let SimdTier::Neon = tier;

    Some(match (format.sample_type, format.channel_count) {
        (_, ChannelCount::Mono) => return None,
        (SampleType::U8, ChannelCount::Rgb) => simd::apply_packed::<u8, 3>,
        (SampleType::U8, ChannelCount::Rgba) => simd::apply_packed::<u8, 4>,
        (SampleType::U16, ChannelCount::Rgb) => simd::apply_packed::<u16, 3>,
        (SampleType::U16, ChannelCount::Rgba) => simd::apply_packed::<u16, 4>,
        (SampleType::F32, ChannelCount::Rgb) => simd::apply_packed::<f32, 3>,
        (SampleType::F32, ChannelCount::Rgba) => simd::apply_packed::<f32, 4>,
    })
}

/// Applies an affine transform to `input`, sampling into `output`.
///
/// Output dimensions come from `output`'s descriptor (they may differ from the
/// input's). Each output pixel center is mapped back through the inverse
/// transform and sampled from the input; sources outside the input read as
/// zero. This mirrors the GPU shader (`shader.wgsl`) so the two backends agree.
///
/// # Panics
/// Panics unless `input` and `output` share a color format. Their dimensions
/// need not match — that is what makes this a resample.
pub(super) fn apply(warp: &AffineWarp, input: &InterleavedImage, output: &mut InterleavedImage) {
    let format = input.desc().color_format;
    assert_eq!(
        format,
        output.desc().color_format,
        "input/output color format mismatch"
    );

    if let Some(kernel) = SimdTier::dispatch(|tier| packed_kernel(tier, format, warp.filter)) {
        unsafe { kernel(warp, input, output) };
        return;
    }

    apply_scalar(warp, input, output);
}

/// The scalar reference, picking the storage type and channel count the format
/// stores pixels in. Split out so the tests can reach it past SIMD dispatch.
pub(super) fn apply_scalar(
    warp: &AffineWarp,
    input: &InterleavedImage,
    output: &mut InterleavedImage,
) {
    let format = input.desc().color_format;
    match (format.sample_type, format.channel_count) {
        (SampleType::U8, ChannelCount::Mono) => apply_typed::<u8, 1>(warp, input, output),
        (SampleType::U8, ChannelCount::Rgb) => apply_typed::<u8, 3>(warp, input, output),
        (SampleType::U8, ChannelCount::Rgba) => apply_typed::<u8, 4>(warp, input, output),
        (SampleType::U16, ChannelCount::Mono) => apply_typed::<u16, 1>(warp, input, output),
        (SampleType::U16, ChannelCount::Rgb) => apply_typed::<u16, 3>(warp, input, output),
        (SampleType::U16, ChannelCount::Rgba) => apply_typed::<u16, 4>(warp, input, output),
        (SampleType::F32, ChannelCount::Mono) => apply_typed::<f32, 1>(warp, input, output),
        (SampleType::F32, ChannelCount::Rgb) => apply_typed::<f32, 3>(warp, input, output),
        (SampleType::F32, ChannelCount::Rgba) => apply_typed::<f32, 4>(warp, input, output),
    }
}

/// Channel values interpolate in their **native** range (u8 `0..=255`, u16
/// `0..=65535`, f32 unchanged), as the GPU shader does, and narrow back by
/// [`Sample::from_f32`], which rounds to nearest with ties to even — the rule WGSL
/// `round` follows on the GPU. Interpolation is linear, so normalizing to `[0, 1]`
/// first would only add a divide and a multiply per channel per tap, and a second
/// rounding. Float output is written unclamped.
fn apply_typed<T, const N: usize>(
    warp: &AffineWarp,
    input: &InterleavedImage,
    output: &mut InterleavedImage,
) where
    T: Sample,
{
    let source = PackedSource::<T>::of(input);
    let out_w = output.desc().size.width;
    let out_stride = output.desc().row_byte_len();

    let inv = warp.inverse();
    let filter = warp.filter;

    par::chunks_mut(output.bytes_mut(), out_stride)
        .enumerate()
        .for_each(|(y, out_row_byte_len)| {
            let out_row: &mut [T] = bytemuck::cast_slice_mut(out_row_byte_len);
            for x in 0..out_w {
                let out_pos = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                let src = inv.transform_point2(out_pos) - Vec2::splat(0.5);
                let rgba = match filter {
                    FilterMode::Nearest => source.nearest::<N>(src),
                    FilterMode::Bilinear => source.bilinear::<N>(src),
                };
                write_pixel::<T, N>(&mut out_row[x * N..x * N + N], rgba);
            }
        });
}

/// Writes the low `N` lanes back into the output pixel's channels.
#[inline]
fn write_pixel<T, const N: usize>(out: &mut [T], rgba: [f32; 4])
where
    T: Sample,
{
    for (dst, &v) in out.iter_mut().zip(rgba.iter()) {
        *dst = T::from_f32(v);
    }
}

#[cfg(test)]
mod tests;
