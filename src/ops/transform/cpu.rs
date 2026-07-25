use bytemuck::Pod;
use glam::Vec2;
use rayon::prelude::*;

use super::{FilterMode, Transform};
use crate::common::color_format::{ChannelSize, ChannelType};
#[cfg(target_arch = "x86_64")]
use crate::cpu_features;
use crate::image::Image;

/// NEON RGB/RGBA bilinear specialization of the scalar path below (aarch64 only).
#[cfg(target_arch = "aarch64")]
mod neon;

/// SSE4.1 RGB/RGBA bilinear specialization of the scalar path below (x86_64).
#[cfg(target_arch = "x86_64")]
mod sse;

/// Applies an affine transform to `input`, sampling into `output`.
///
/// Output dimensions come from `output`'s descriptor (they may differ from the
/// input's). Each output pixel center is mapped back through the inverse
/// transform and sampled from the input; sources outside the input read as
/// zero. This mirrors the GPU shader (`shader.wgsl`) so the two backends agree.
///
/// # Panics
/// Panics if `input` and `output` have different color formats.
pub(super) fn apply(transform: &Transform, input: &Image, output: &mut Image) {
    assert_eq!(
        input.desc().color_format,
        output.desc().color_format,
        "color format mismatch"
    );

    let fmt = input.desc().color_format;
    let channels = fmt.channel_count.channel_count() as usize;

    // RGB/RGBA bilinear vectorize and are bit-identical to the scalar reference
    // (cross-checked). L stays scalar (gather-bound — SIMD measured slower), and
    // nearest is a near-memcpy the scalar path already nails.
    #[cfg(target_arch = "aarch64")]
    if channels >= 3 && transform.filter == FilterMode::Bilinear {
        use ChannelSize::{_8bit, _16bit, _32bit};
        use ChannelType::{Float, UInt};
        match (fmt.channel_size, fmt.channel_type, channels) {
            (_8bit, UInt, 4) => return neon::apply_packed::<u8, 4>(transform, input, output),
            (_8bit, UInt, 3) => return neon::apply_packed::<u8, 3>(transform, input, output),
            (_16bit, UInt, 4) => return neon::apply_packed::<u16, 4>(transform, input, output),
            (_16bit, UInt, 3) => return neon::apply_packed::<u16, 3>(transform, input, output),
            (_32bit, Float, 4) => return neon::apply_packed::<f32, 4>(transform, input, output),
            (_32bit, Float, 3) => return neon::apply_packed::<f32, 3>(transform, input, output),
            _ => {}
        }
    }

    #[cfg(target_arch = "x86_64")]
    if channels >= 3 && transform.filter == FilterMode::Bilinear && cpu_features::has_sse4_1() {
        use ChannelSize::{_8bit, _16bit, _32bit};
        use ChannelType::{Float, UInt};
        match (fmt.channel_size, fmt.channel_type, channels) {
            (_8bit, UInt, 4) => return sse::apply_packed::<u8, 4>(transform, input, output),
            (_8bit, UInt, 3) => return sse::apply_packed::<u8, 3>(transform, input, output),
            (_16bit, UInt, 4) => return sse::apply_packed::<u16, 4>(transform, input, output),
            (_16bit, UInt, 3) => return sse::apply_packed::<u16, 3>(transform, input, output),
            (_32bit, Float, 4) => return sse::apply_packed::<f32, 4>(transform, input, output),
            (_32bit, Float, 3) => return sse::apply_packed::<f32, 3>(transform, input, output),
            _ => {}
        }
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::color_format::{ALL_FORMATS, ColorFormat};
    use crate::common::image_diff::pixels_equal;
    use crate::common::internals::create_test_image;
    use crate::image::ImageDesc;
    use crate::ops::transform::{FilterMode, Transform};

    fn image_u8(width: usize, height: usize, fmt: ColorFormat, bytes: Vec<u8>) -> Image {
        Image::new_with_data(ImageDesc::new(width, height, fmt), bytes).unwrap()
    }

    fn image_f32(width: usize, height: usize, fmt: ColorFormat, values: &[f32]) -> Image {
        let bytes: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
        Image::new_with_data(ImageDesc::new(width, height, fmt), bytes).unwrap()
    }

    /// Identity transform is a bit-exact copy for every format and both filters.
    /// The normalize/denormalize round-trip is exact for all u8/u16 values, and
    /// bilinear at integer source coordinates collapses to the source pixel.
    #[test]
    fn test_identity_is_exact_all_formats() {
        for &filter in &[FilterMode::Nearest, FilterMode::Bilinear] {
            for &format in ALL_FORMATS {
                // Non-power-of-two dimensions to exercise edge rows/columns.
                let input = create_test_image(format, 13, 7, 0);
                let mut output = Image::new_black(input.desc()).unwrap();

                Transform::new()
                    .filter(filter)
                    .apply_cpu(&input, &mut output);

                assert!(
                    pixels_equal(&input, &output),
                    "identity changed pixels for {format} with {filter:?}"
                );
            }
        }
    }

    /// Integer translation shifts pixels exactly and zero-fills the exposed edge.
    /// Input row [10,20,30,40] translated +1 in x becomes [0,10,20,30].
    #[test]
    fn test_integer_translate_shifts_pixels() {
        let input = image_u8(4, 1, ColorFormat::L_U8, vec![10, 20, 30, 40]);
        let mut output = Image::new_black(input.desc()).unwrap();

        Transform::new()
            .translate(Vec2::new(1.0, 0.0))
            .filter(FilterMode::Bilinear)
            .apply_cpu(&input, &mut output);

        assert_eq!(output.bytes(), &[0, 10, 20, 30]);
    }

    /// Nearest-neighbor 2x downscale picks even source columns.
    /// src_x = 2*ox + 0.5, round-ties-to-even → 0, 2, ... so [10,20,30,40] → [10,30].
    #[test]
    fn test_nearest_downscale_picks_even_columns() {
        let input = image_u8(4, 1, ColorFormat::L_U8, vec![10, 20, 30, 40]);
        let mut output = Image::new_black(ImageDesc::new(2, 1, ColorFormat::L_U8)).unwrap();

        Transform::new()
            .scale(Vec2::new(0.5, 0.5))
            .filter(FilterMode::Nearest)
            .apply_cpu(&input, &mut output);

        assert_eq!(output.bytes(), &[10, 30]);
    }

    /// Bilinear with a half-pixel translation averages adjacent samples 50/50.
    /// src_x = ox - 0.5 → floor ox-1, fx 0.5. Powers-of-two weights keep the f32
    /// math exact: [0.25,0.75,0.5] → [0.125, 0.5, 0.625].
    #[test]
    fn test_bilinear_half_pixel_average() {
        let input = image_f32(3, 1, ColorFormat::L_F32, &[0.25, 0.75, 0.5]);
        let mut output = Image::new_black(ImageDesc::new(3, 1, ColorFormat::L_F32)).unwrap();

        Transform::new()
            .translate(Vec2::new(0.5, 0.0))
            .filter(FilterMode::Bilinear)
            .apply_cpu(&input, &mut output);

        let out: &[f32] = bytemuck::cast_slice(output.bytes());
        assert_eq!(out, &[0.125, 0.5, 0.625]);
    }

    /// RGBA alpha is interpolated like any other channel, not forced opaque.
    /// Bilinear at the midpoint of alpha 0 and alpha 1 yields exactly 0.5.
    #[test]
    fn test_rgba_alpha_is_interpolated() {
        let input = image_f32(
            2,
            1,
            ColorFormat::RGBA_F32,
            &[0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0],
        );
        let mut output = Image::new_black(ImageDesc::new(2, 1, ColorFormat::RGBA_F32)).unwrap();

        Transform::new()
            .translate(Vec2::new(0.5, 0.0))
            .filter(FilterMode::Bilinear)
            .apply_cpu(&input, &mut output);

        let out: &[f32] = bytemuck::cast_slice(output.bytes());
        // Pixel 0 maps fully out of bounds; pixel 1 is the 0/1 midpoint.
        assert_eq!(out, &[0.0, 0.0, 0.0, 0.0, 0.5, 0.5, 0.5, 0.5]);
    }

    /// A transform that maps every output pixel outside the input yields zeros
    /// (including alpha), for every format.
    #[test]
    fn test_fully_out_of_bounds_is_zero_all_formats() {
        for &format in ALL_FORMATS {
            let input = create_test_image(format, 3, 3, 0);
            let mut output = Image::new_black(input.desc()).unwrap();

            Transform::new()
                .translate(Vec2::new(1000.0, 1000.0))
                .apply_cpu(&input, &mut output);

            assert!(
                output.bytes().iter().all(|&b| b == 0),
                "off-screen transform left non-zero pixels for {format}"
            );
        }
    }

    /// Nearest and bilinear produce different results when upsampling a gradient,
    /// confirming the filter parameter actually changes behavior.
    #[test]
    fn test_filter_modes_differ_on_upscale() {
        let input = image_u8(2, 1, ColorFormat::L_U8, vec![0, 100]);
        let out_desc = ImageDesc::new(4, 1, ColorFormat::L_U8);

        let mut nearest = Image::new_black(out_desc).unwrap();
        Transform::new()
            .scale(Vec2::new(2.0, 2.0))
            .filter(FilterMode::Nearest)
            .apply_cpu(&input, &mut nearest);

        let mut bilinear = Image::new_black(out_desc).unwrap();
        Transform::new()
            .scale(Vec2::new(2.0, 2.0))
            .filter(FilterMode::Bilinear)
            .apply_cpu(&input, &mut bilinear);

        // Nearest replicates source samples; bilinear ramps between them.
        assert_eq!(nearest.bytes(), &[0, 0, 100, 100]);
        assert_ne!(nearest.bytes(), bilinear.bytes());
    }

    /// Every NEON packed bilinear path (RGB, RGBA) is bit-identical to the scalar
    /// reference, on a rotation and a non-multiple-of-4 width that hit interior,
    /// edge, and out-of-bounds taps.
    #[cfg(target_arch = "aarch64")]
    #[test]
    fn test_neon_matches_scalar() {
        use ChannelSize::{_8bit, _16bit, _32bit};
        use ChannelType::{Float, UInt};
        let center = Vec2::new(18.5, 9.5);
        for &format in ALL_FORMATS {
            let cc = format.channel_count.channel_count();
            if cc == 1 {
                continue; // L has no NEON path.
            }
            let input = create_test_image(format, 37, 19, 7);
            let transform = Transform::new()
                .rotate_around(0.7, center)
                .filter(FilterMode::Bilinear);

            let mut scalar = Image::new_black(input.desc()).unwrap();
            let mut simd = Image::new_black(input.desc()).unwrap();

            match (format.channel_size, format.channel_type, cc) {
                (_8bit, UInt, 4) => {
                    apply_typed::<u8, 4>(&transform, &input, &mut scalar);
                    neon::apply_packed::<u8, 4>(&transform, &input, &mut simd);
                }
                (_8bit, UInt, 3) => {
                    apply_typed::<u8, 3>(&transform, &input, &mut scalar);
                    neon::apply_packed::<u8, 3>(&transform, &input, &mut simd);
                }
                (_16bit, UInt, 4) => {
                    apply_typed::<u16, 4>(&transform, &input, &mut scalar);
                    neon::apply_packed::<u16, 4>(&transform, &input, &mut simd);
                }
                (_16bit, UInt, 3) => {
                    apply_typed::<u16, 3>(&transform, &input, &mut scalar);
                    neon::apply_packed::<u16, 3>(&transform, &input, &mut simd);
                }
                (_32bit, Float, 4) => {
                    apply_typed::<f32, 4>(&transform, &input, &mut scalar);
                    neon::apply_packed::<f32, 4>(&transform, &input, &mut simd);
                }
                (_32bit, Float, 3) => {
                    apply_typed::<f32, 3>(&transform, &input, &mut scalar);
                    neon::apply_packed::<f32, 3>(&transform, &input, &mut simd);
                }
                _ => unreachable!(),
            }

            assert!(
                pixels_equal(&scalar, &simd),
                "NEON diverged from scalar for {format}"
            );
        }
    }

    /// Every SSE4.1 packed bilinear path (RGB, RGBA) is bit-identical to the
    /// scalar reference, on a rotation and a non-multiple-of-4 width that hit
    /// interior, edge, and out-of-bounds taps. Skips if the CPU lacks SSE4.1.
    #[cfg(target_arch = "x86_64")]
    #[test]
    fn test_sse_matches_scalar() {
        if !cpu_features::has_sse4_1() {
            return;
        }
        use ChannelSize::{_8bit, _16bit, _32bit};
        use ChannelType::{Float, UInt};
        let center = Vec2::new(18.5, 9.5);
        for &format in ALL_FORMATS {
            let cc = format.channel_count.channel_count();
            if cc == 1 {
                continue; // L has no SSE path.
            }
            let input = create_test_image(format, 37, 19, 7);
            let transform = Transform::new()
                .rotate_around(0.7, center)
                .filter(FilterMode::Bilinear);

            let mut scalar = Image::new_black(input.desc()).unwrap();
            let mut simd = Image::new_black(input.desc()).unwrap();

            match (format.channel_size, format.channel_type, cc) {
                (_8bit, UInt, 4) => {
                    apply_typed::<u8, 4>(&transform, &input, &mut scalar);
                    sse::apply_packed::<u8, 4>(&transform, &input, &mut simd);
                }
                (_8bit, UInt, 3) => {
                    apply_typed::<u8, 3>(&transform, &input, &mut scalar);
                    sse::apply_packed::<u8, 3>(&transform, &input, &mut simd);
                }
                (_16bit, UInt, 4) => {
                    apply_typed::<u16, 4>(&transform, &input, &mut scalar);
                    sse::apply_packed::<u16, 4>(&transform, &input, &mut simd);
                }
                (_16bit, UInt, 3) => {
                    apply_typed::<u16, 3>(&transform, &input, &mut scalar);
                    sse::apply_packed::<u16, 3>(&transform, &input, &mut simd);
                }
                (_32bit, Float, 4) => {
                    apply_typed::<f32, 4>(&transform, &input, &mut scalar);
                    sse::apply_packed::<f32, 4>(&transform, &input, &mut simd);
                }
                (_32bit, Float, 3) => {
                    apply_typed::<f32, 3>(&transform, &input, &mut scalar);
                    sse::apply_packed::<f32, 3>(&transform, &input, &mut simd);
                }
                _ => unreachable!(),
            }

            assert!(
                pixels_equal(&scalar, &simd),
                "SSE diverged from scalar for {format}"
            );
        }
    }

    #[cfg(feature = "wgpu")]
    mod gpu_cross_check {
        use super::*;
        use crate::common::image_diff::max_pixel_diff;
        use crate::common::internals::{gpu::test_gpu, load_lena_rgba_u8_61x38};
        use crate::gpu::gpu_image::GpuImage;
        use crate::ops::transform::pipeline::GpuTransformPipeline;

        /// Integer translation is exact on both backends, so CPU and GPU outputs
        /// must be bit-identical.
        #[test]
        fn test_cpu_matches_gpu_integer_translate() {
            let Some(gpu) = test_gpu() else {
                return;
            };
            let pipeline = GpuTransformPipeline::new(&gpu).unwrap();
            let input = load_lena_rgba_u8_61x38();
            let transform = Transform::new().translate(Vec2::new(5.0, 3.0));

            let mut out_cpu = Image::new_black(input.desc()).unwrap();
            transform.apply_cpu(&input, &mut out_cpu);

            let gpu_in = GpuImage::from_image(&gpu, &input);
            let mut gpu_out = GpuImage::new_empty(&gpu, input.desc());
            transform.apply_gpu(&gpu, &pipeline, &gpu_in, &mut gpu_out);
            let out_gpu = gpu_out.to_image(&gpu).unwrap();

            assert_eq!(
                max_pixel_diff(&out_cpu, &out_gpu),
                0.0,
                "CPU and GPU disagree on integer translation"
            );
        }

        /// A 2x bilinear downscale lands on half-integer source coordinates, so
        /// both backends sample the same taps with weight 0.5; only the final
        /// rounding can differ, by at most one quantization step.
        #[test]
        fn test_cpu_matches_gpu_bilinear_downscale() {
            let Some(gpu) = test_gpu() else {
                return;
            };
            let pipeline = GpuTransformPipeline::new(&gpu).unwrap();
            let input = load_lena_rgba_u8_61x38();
            let out_desc = ImageDesc::new(30, 19, ColorFormat::RGBA_U8);
            let transform = Transform::new()
                .scale(Vec2::new(0.5, 0.5))
                .filter(FilterMode::Bilinear);

            let mut out_cpu = Image::new_black(out_desc).unwrap();
            transform.apply_cpu(&input, &mut out_cpu);

            let gpu_in = GpuImage::from_image(&gpu, &input);
            let mut gpu_out = GpuImage::new_empty(&gpu, out_desc);
            transform.apply_gpu(&gpu, &pipeline, &gpu_in, &mut gpu_out);
            let out_gpu = gpu_out.to_image(&gpu).unwrap();

            let diff = max_pixel_diff(&out_cpu, &out_gpu);
            assert!(
                diff < 2.0 / 255.0,
                "CPU and GPU bilinear downscale differ by {diff} (> 1 LSB)"
            );
        }
    }
}
