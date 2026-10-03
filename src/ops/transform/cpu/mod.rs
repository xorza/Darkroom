/// NEON RGB/RGBA bilinear specialization of the scalar path below (aarch64 only).
#[cfg(target_arch = "aarch64")]
mod neon;

/// SSE4.1 RGB/RGBA bilinear specialization of the scalar path below (x86_64).
#[cfg(target_arch = "x86_64")]
mod sse;

use std::array;

use glam::Vec2;
use rayon::prelude::*;

use crate::common::color_format::{ChannelCount, ColorFormat, SampleType};
use crate::common::sample::Sample;
use crate::image::Image;
use crate::ops::transform::{FilterMode, Transform};
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
use crate::simd_tier::SimdTier;

/// A SIMD kernel: the packed RGB/RGBA bilinear specialization of [`apply_typed`]
/// for one storage type and channel count.
///
/// # Safety
/// The running CPU must support the feature the kernel was compiled for;
/// [`packed_kernel`] at a supported [`SimdTier`] is what establishes that.
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
type PackedKernel = unsafe fn(&Transform, &Image, &mut Image);

/// The SIMD kernel `tier` has for `format` under `filter`, or `None` when the
/// tier or the combination has no vector path — callers then take the scalar
/// reference.
///
/// RGB/RGBA bilinear vectorize and are bit-identical to the scalar reference
/// (cross-checked). L stays scalar (gather-bound — SIMD measured slower), and
/// nearest is a near-memcpy the scalar path already nails.
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
fn packed_kernel(tier: SimdTier, format: ColorFormat, filter: FilterMode) -> Option<PackedKernel> {
    #[cfg(target_arch = "aarch64")]
    use crate::ops::transform::cpu::neon as simd;
    #[cfg(target_arch = "x86_64")]
    use crate::ops::transform::cpu::sse as simd;

    if filter != FilterMode::Bilinear {
        return None;
    }

    #[cfg(target_arch = "x86_64")]
    if tier < SimdTier::Sse41 {
        return None;
    }
    #[cfg(target_arch = "aarch64")]
    let SimdTier::Neon = tier;

    Some(match (format.sample_type, format.channel_count) {
        (_, ChannelCount::L) => return None,
        (SampleType::U8, ChannelCount::Rgb) => simd::apply_packed::<u8, 3> as PackedKernel,
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
pub(super) fn apply(transform: &Transform, input: &Image, output: &mut Image) {
    let format = input.desc().color_format;
    assert_eq!(
        format,
        output.desc().color_format,
        "input/output color format mismatch"
    );

    #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
    if let Some(kernel) =
        SimdTier::widest().and_then(|tier| packed_kernel(tier, format, transform.filter))
    {
        // SAFETY: the kernel is the widest supported tier's.
        unsafe { kernel(transform, input, output) };
        return;
    }

    apply_scalar(transform, input, output);
}

/// The scalar reference, picking the storage type and channel count the format
/// stores pixels in. Split out so the tests can reach it past SIMD dispatch.
fn apply_scalar(transform: &Transform, input: &Image, output: &mut Image) {
    let format = input.desc().color_format;
    match (format.sample_type, format.channel_count) {
        (SampleType::U8, ChannelCount::L) => apply_typed::<u8, 1>(transform, input, output),
        (SampleType::U8, ChannelCount::Rgb) => apply_typed::<u8, 3>(transform, input, output),
        (SampleType::U8, ChannelCount::Rgba) => apply_typed::<u8, 4>(transform, input, output),
        (SampleType::U16, ChannelCount::L) => apply_typed::<u16, 1>(transform, input, output),
        (SampleType::U16, ChannelCount::Rgb) => apply_typed::<u16, 3>(transform, input, output),
        (SampleType::U16, ChannelCount::Rgba) => apply_typed::<u16, 4>(transform, input, output),
        (SampleType::F32, ChannelCount::L) => apply_typed::<f32, 1>(transform, input, output),
        (SampleType::F32, ChannelCount::Rgb) => apply_typed::<f32, 3>(transform, input, output),
        (SampleType::F32, ChannelCount::Rgba) => apply_typed::<f32, 4>(transform, input, output),
    }
}

/// Channel values interpolate in their **native** range (u8 `0..=255`, u16
/// `0..=65535`, f32 unchanged), as the GPU shader does, and narrow back by
/// [`Sample::from_f32`], which rounds to nearest with ties to even — the rule WGSL
/// `round` follows on the GPU. Interpolation is linear, so normalizing to `[0, 1]`
/// first would only add a divide and a multiply per channel per tap, and a second
/// rounding. Float output is written unclamped.
fn apply_typed<T, const N: usize>(transform: &Transform, input: &Image, output: &mut Image)
where
    T: Sample,
{
    let in_w = input.desc().width;
    let in_h = input.desc().height;
    let out_w = output.desc().width;
    let out_stride = output.desc().row_bytes();

    let in_pixels: &[T] = bytemuck::cast_slice(input.bytes());

    let inv = transform.inverse();
    let filter = transform.filter;

    output
        .bytes_mut()
        .par_chunks_mut(out_stride)
        .enumerate()
        .for_each(|(y, out_row_bytes)| {
            let out_row: &mut [T] = bytemuck::cast_slice_mut(out_row_bytes);
            for x in 0..out_w {
                let out_pos = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                let src = inv.transform_point2(out_pos) - Vec2::splat(0.5);
                let rgba = match filter {
                    FilterMode::Nearest => sample_nearest::<T, N>(in_pixels, in_w, in_h, src),
                    FilterMode::Bilinear => sample_bilinear::<T, N>(in_pixels, in_w, in_h, src),
                };
                write_pixel::<T, N>(&mut out_row[x * N..x * N + N], rgba);
            }
        });
}

/// Reads the `N` channels of the pixel at integer `(x, y)` into the low lanes of
/// an `[f32; 4]` (unused lanes stay zero); out-of-bounds reads as all-zero. Only
/// the low `N` lanes are ever written back, so the padding never affects output.
#[inline]
fn read_pixel<T, const N: usize>(
    pixels: &[T],
    width: usize,
    height: usize,
    x: i32,
    y: i32,
) -> [f32; 4]
where
    T: Sample,
{
    let Some(pixel) = pixel_index(width, height, x, y) else {
        return [0.0; 4];
    };
    let base = pixel * N;
    let mut px = [0.0f32; 4];
    for (lane, &raw) in px.iter_mut().zip(&pixels[base..base + N]) {
        *lane = raw.to_f32();
    }
    px
}

/// The index of pixel `(x, y)` in a `width × height` image, or `None` outside it.
#[inline]
pub(super) fn pixel_index(width: usize, height: usize, x: i32, y: i32) -> Option<usize> {
    let (x, y) = (usize::try_from(x).ok()?, usize::try_from(y).ok()?);
    (x < width && y < height).then_some(y * width + x)
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

#[inline]
fn sample_nearest<T, const N: usize>(
    pixels: &[T],
    width: usize,
    height: usize,
    pos: Vec2,
) -> [f32; 4]
where
    T: Sample,
{
    // `round_ties_even` matches WGSL `round`, which rounds halves to even.
    let x = pos.x.round_ties_even() as i32;
    let y = pos.y.round_ties_even() as i32;
    read_pixel::<T, N>(pixels, width, height, x, y)
}

#[inline]
fn sample_bilinear<T, const N: usize>(
    pixels: &[T],
    width: usize,
    height: usize,
    pos: Vec2,
) -> [f32; 4]
where
    T: Sample,
{
    let fx0 = pos.x.floor();
    let fy0 = pos.y.floor();
    let fx = pos.x - fx0;
    let fy = pos.y - fy0;
    let x0 = fx0 as i32;
    let y0 = fy0 as i32;

    let c00 = read_pixel::<T, N>(pixels, width, height, x0, y0);
    let c10 = read_pixel::<T, N>(pixels, width, height, x0 + 1, y0);
    let c01 = read_pixel::<T, N>(pixels, width, height, x0, y0 + 1);
    let c11 = read_pixel::<T, N>(pixels, width, height, x0 + 1, y0 + 1);

    let c0 = mix(c00, c10, fx);
    let c1 = mix(c01, c11, fx);
    mix(c0, c1, fy)
}

/// `mix(a, b, t) = a * (1 - t) + b * t`, per channel — matching WGSL `mix`.
#[inline]
fn mix(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    array::from_fn(|i| a[i] * (1.0 - t) + b[i] * t)
}

#[cfg(test)]
mod tests;
