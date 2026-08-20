/// NEON RGB/RGBA bilinear specialization of the scalar path below (aarch64 only).
#[cfg(target_arch = "aarch64")]
mod neon;

/// SSE4.1 RGB/RGBA bilinear specialization of the scalar path below (x86_64).
#[cfg(target_arch = "x86_64")]
mod sse;

#[cfg(test)]
mod tests;

use bytemuck::Pod;
use glam::Vec2;
use rayon::prelude::*;

use crate::common::color_format::{ChannelSize, ChannelType, ColorFormat};
#[cfg(target_arch = "x86_64")]
use crate::cpu_features;
use crate::image::Image;
use crate::ops::transform::{FilterMode, Transform};

/// A SIMD kernel: the packed RGB/RGBA bilinear specialization of [`apply_typed`]
/// for one storage type and channel count.
///
/// # Safety
/// The running CPU must support the feature the kernel was compiled for;
/// [`packed_kernel`] is what establishes that.
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
type PackedKernel = unsafe fn(&Transform, &Image, &mut Image);

/// The SIMD kernel for `format` under `filter` on this arch, or `None` when the
/// CPU lacks the feature or the combination has no vector path — callers then
/// take the scalar reference.
///
/// RGB/RGBA bilinear vectorize and are bit-identical to the scalar reference
/// (cross-checked). L stays scalar (gather-bound — SIMD measured slower), and
/// nearest is a near-memcpy the scalar path already nails.
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
fn packed_kernel(format: ColorFormat, filter: FilterMode) -> Option<PackedKernel> {
    #[cfg(target_arch = "aarch64")]
    use crate::ops::transform::cpu::neon as simd;
    #[cfg(target_arch = "x86_64")]
    use crate::ops::transform::cpu::sse as simd;

    if filter != FilterMode::Bilinear {
        return None;
    }

    #[cfg(target_arch = "x86_64")]
    if !cpu_features::has_sse4_1() {
        return None;
    }

    use ChannelSize::{_8bit, _16bit, _32bit};
    use ChannelType::{Float, UInt};
    let channels = format.channel_count.channel_count();
    Some(match (format.channel_size, format.channel_type, channels) {
        (_8bit, UInt, 3) => simd::apply_packed::<u8, 3> as PackedKernel,
        (_8bit, UInt, 4) => simd::apply_packed::<u8, 4> as PackedKernel,
        (_16bit, UInt, 3) => simd::apply_packed::<u16, 3> as PackedKernel,
        (_16bit, UInt, 4) => simd::apply_packed::<u16, 4> as PackedKernel,
        (_32bit, Float, 3) => simd::apply_packed::<f32, 3> as PackedKernel,
        (_32bit, Float, 4) => simd::apply_packed::<f32, 4> as PackedKernel,
        _ => return None,
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
    if let Some(kernel) = packed_kernel(format, transform.filter) {
        // SAFETY: `packed_kernel` verified this CPU has the kernel's feature.
        unsafe { kernel(transform, input, output) };
        return;
    }

    apply_scalar(transform, input, output);
}

/// The scalar reference, picking the storage type and channel count the format
/// stores pixels in. Split out so the tests can reach it past SIMD dispatch.
fn apply_scalar(transform: &Transform, input: &Image, output: &mut Image) {
    let fmt = input.desc().color_format;
    let channels = fmt.channel_count.channel_count();
    match (fmt.channel_size, fmt.channel_type, channels) {
        (ChannelSize::_8bit, ChannelType::UInt, 1) => {
            apply_typed::<u8, 1>(transform, input, output)
        }
        (ChannelSize::_8bit, ChannelType::UInt, 3) => {
            apply_typed::<u8, 3>(transform, input, output)
        }
        (ChannelSize::_8bit, ChannelType::UInt, 4) => {
            apply_typed::<u8, 4>(transform, input, output)
        }
        (ChannelSize::_16bit, ChannelType::UInt, 1) => {
            apply_typed::<u16, 1>(transform, input, output)
        }
        (ChannelSize::_16bit, ChannelType::UInt, 3) => {
            apply_typed::<u16, 3>(transform, input, output)
        }
        (ChannelSize::_16bit, ChannelType::UInt, 4) => {
            apply_typed::<u16, 4>(transform, input, output)
        }
        (ChannelSize::_32bit, ChannelType::Float, 1) => {
            apply_typed::<f32, 1>(transform, input, output)
        }
        (ChannelSize::_32bit, ChannelType::Float, 3) => {
            apply_typed::<f32, 3>(transform, input, output)
        }
        (ChannelSize::_32bit, ChannelType::Float, 4) => {
            apply_typed::<f32, 4>(transform, input, output)
        }
        _ => unreachable!("unsupported color format for transform: {fmt:?}"),
    }
}

/// A pixel channel element converted to/from `f32` in its **native** value
/// range (u8 `0..=255`, u16 `0..=65535`, f32 unchanged).
///
/// The GPU shader normalizes to `[0, 1]` before interpolating and rescales on
/// write; on the CPU that round-trip is pure overhead, because interpolation is
/// linear (`mix(a/M, b/M, t) * M == mix(a, b, t)`) so the `/M` and `*M` cancel.
/// Interpolating in the native range drops one divide and one multiply per
/// channel per tap — and is marginally more accurate (no double rounding).
trait TransformElem: Pod + Send + Sync {
    fn to_f32(self) -> f32;
    fn from_f32(v: f32) -> Self;
}

impl TransformElem for u8 {
    #[inline]
    fn to_f32(self) -> f32 {
        self as f32
    }
    #[inline]
    fn from_f32(v: f32) -> Self {
        // Truncate toward zero, matching the shader's `u32(clamp(...))`.
        v.clamp(0.0, 255.0) as Self
    }
}

impl TransformElem for u16 {
    #[inline]
    fn to_f32(self) -> f32 {
        self as f32
    }
    #[inline]
    fn from_f32(v: f32) -> Self {
        v.clamp(0.0, 65535.0) as Self
    }
}

impl TransformElem for f32 {
    #[inline]
    fn to_f32(self) -> f32 {
        self
    }
    #[inline]
    fn from_f32(v: f32) -> Self {
        // Float output is written unclamped, matching the shader.
        v
    }
}

fn apply_typed<T, const N: usize>(transform: &Transform, input: &Image, output: &mut Image)
where
    T: TransformElem,
{
    let in_w = input.desc().width;
    let in_h = input.desc().height;
    let out_w = output.desc().width;
    let out_stride = output.desc().row_bytes();

    let in_pixels: &[T] = bytemuck::cast_slice(input.bytes());

    let inv = transform.transform.inverse();
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
    T: TransformElem,
{
    if x < 0 || y < 0 || x >= width as i32 || y >= height as i32 {
        return [0.0; 4];
    }
    let base = (y as usize * width + x as usize) * N;
    let mut px = [0.0f32; 4];
    for (lane, &raw) in px.iter_mut().zip(&pixels[base..base + N]) {
        *lane = raw.to_f32();
    }
    px
}

/// Writes the low `N` lanes back into the output pixel's channels.
#[inline]
fn write_pixel<T, const N: usize>(out: &mut [T], rgba: [f32; 4])
where
    T: TransformElem,
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
    T: TransformElem,
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
    T: TransformElem,
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
    std::array::from_fn(|i| a[i] * (1.0 - t) + b[i] * t)
}
