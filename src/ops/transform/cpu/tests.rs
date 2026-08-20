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

/// Every SIMD kernel this CPU dispatches to is bit-identical to the scalar
/// reference it specializes: the vector path interpolates in the same order and
/// the same native units, so there is nothing to round differently.
///
/// The rotation and the non-multiple-of-4 width hit interior, edge and
/// out-of-bounds taps. Formats with no kernel here — L, and everything when the
/// CPU lacks SSE4.1 — dispatch to the reference itself and are skipped.
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
#[test]
fn simd_matches_scalar_reference() {
    let transform = Transform::new()
        .rotate_around(0.7, Vec2::new(18.5, 9.5))
        .filter(FilterMode::Bilinear);

    for &format in ALL_FORMATS {
        let Some(kernel) = packed_kernel(format, transform.filter) else {
            continue;
        };
        let input = create_test_image(format, 37, 19, 7);
        let mut scalar = Image::new_black(input.desc()).unwrap();
        let mut simd = Image::new_black(input.desc()).unwrap();

        apply_scalar(&transform, &input, &mut scalar);
        // SAFETY: `packed_kernel` verified this CPU has the kernel's feature.
        unsafe { kernel(&transform, &input, &mut simd) };

        assert!(
            pixels_equal(&scalar, &simd),
            "SIMD diverged from the scalar reference for {format}"
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
