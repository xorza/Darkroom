use crate::math::vec2us::Vec2us;
use crate::stacking::registration::config::{self, InterpolationMethod, WarpParams};
use crate::stacking::registration::resample::kernel::test_support as kernel_test_support;
use crate::stacking::registration::resample::{plane, quality};
use crate::stacking::registration::transform::{Transform, WarpTransform};
use glam::{DVec2, Vec2};
use imaginarium::Buffer2;

#[test]
fn test_warp_identity_preserves_image() {
    let input: Vec<f32> = (0..16).map(|i| i as f32).collect();
    let input_buf = Buffer2::new(4, 4, input.clone());

    let mut output = Buffer2::new_default(4, 4);
    plane::warp(
        &input_buf,
        &mut output,
        &WarpTransform::new(Transform::identity()),
        &config::test_support::warp_params(InterpolationMethod::Bilinear),
    );

    for (i, (&inp, &out)) in input.iter().zip(output.iter()).enumerate() {
        assert!(
            (inp - out).abs() < 0.1,
            "Pixel {i}: expected {inp}, got {out}"
        );
    }
}

#[test]
fn test_warp_integer_translation() {
    // Translation by (1,1): output[p] = input[T(p)] = input[p + (1,1)]
    // So input(1,1)=5 appears at output(0,0)
    let mut input = vec![0.0f32; 16];
    input[5] = 1.0; // Position (1, 1) in a 4x4 grid: index = 1*4+1 = 5
    let input_buf = Buffer2::new(4, 4, input);

    let transform = Transform::translation(DVec2::new(1.0, 1.0));

    let mut output = Buffer2::new_default(4, 4);
    plane::warp(
        &input_buf,
        &mut output,
        &WarpTransform::new(transform),
        &config::test_support::warp_params(InterpolationMethod::Bilinear),
    );

    // output(0,0) samples input(1,1) = 1.0
    assert!(
        (output[0] - 1.0).abs() < 0.01,
        "Expected 1.0 at output(0,0), got {}",
        output[0]
    );
    // output(1,1) samples input(2,2) = 0.0
    assert!(
        output[5] < 0.01,
        "Expected 0.0 at output(1,1), got {}",
        output[5]
    );
}

#[test]
fn test_plane_warp_lanczos3_identity() {
    let width = 32;
    let height = 32;
    let input: Vec<f32> = (0..width * height).map(|i| (i as f32) / 1024.0).collect();
    let input_buf = Buffer2::new(width, height, input);

    let mut output = Buffer2::new_filled(width, height, 0.0);
    plane::warp(
        &input_buf,
        &mut output,
        &WarpTransform::new(Transform::identity()),
        &config::test_support::warp_params(InterpolationMethod::Lanczos3),
    );

    // Interior pixels should match input closely
    for y in 4..height - 4 {
        for x in 4..width - 4 {
            let expected = input_buf[(x, y)];
            let actual = output[(x, y)];
            assert!(
                (actual - expected).abs() < 0.02,
                "Identity mismatch at ({x}, {y}): {actual} vs {expected}"
            );
        }
    }
}

#[test]
fn test_plane_warp_lanczos3_integer_translation() {
    let width = 64;
    let height = 64;
    let input: Vec<f32> = (0..width * height).map(|i| (i as f32) / 4096.0).collect();
    let input_buf = Buffer2::new(width, height, input);
    // output[p] = input[p + (5,3)]
    let transform = Transform::translation(DVec2::new(5.0, 3.0));

    let mut output = Buffer2::new_filled(width, height, 0.0);
    plane::warp(
        &input_buf,
        &mut output,
        &WarpTransform::new(transform),
        &config::test_support::warp_params(InterpolationMethod::Lanczos3),
    );

    for y in 8..height - 8 {
        for x in 5..width - 15 {
            let expected = input_buf[(x + 5, y + 3)];
            let actual = output[(x, y)];
            assert!(
                (actual - expected).abs() < 0.02,
                "Translation mismatch at ({x}, {y}): {actual} vs {expected}"
            );
        }
    }
}

#[test]
fn test_plane_warp_lanczos3_matches_per_pixel() {
    // Verify the optimized plane warp Lanczos3 path matches the per-pixel reference.
    let width = 32;
    let height = 32;
    let input: Vec<f32> = (0..width * height)
        .map(|i| ((i as f32 * 0.037).sin() + 1.0) * 0.5)
        .collect();
    let input_buf = Buffer2::new(width, height, input);
    let transform = Transform::similarity(DVec2::new(16.0, 16.0), 0.03, 1.02);

    let mut output = Buffer2::new_filled(width, height, 0.0);
    let params = config::test_support::warp_params(InterpolationMethod::Lanczos3);
    plane::warp(
        &input_buf,
        &mut output,
        &WarpTransform::new(transform),
        &params,
    );

    for y in 0..height {
        for x in 0..width {
            let src = transform.apply(DVec2::new(x as f64, y as f64));
            let expected = kernel_test_support::interpolate(
                &input_buf,
                Vec2::new(src.x as f32, src.y as f32),
                &params,
            );
            let actual = output[(x, y)];
            assert!(
                (actual - expected).abs() < 1e-4,
                "Mismatch at ({x}, {y}): plane warp={actual} vs interpolate={expected}"
            );
        }
    }
}

// The generic warp loop (used for Bicubic, Lanczos2, Lanczos4, Nearest)
// uses incremental stepping for linear transforms. These tests verify
// that the stepped output matches per-pixel transform.apply() exactly.

/// Reference per-pixel warp used to validate incremental stepping.
fn warp_per_pixel_reference(
    input: &Buffer2<f32>,
    output: &mut Buffer2<f32>,
    warp_transform: &WarpTransform,
    params: &WarpParams,
) {
    let width = input.width();
    let height = input.height();
    for y in 0..height {
        for x in 0..width {
            let src = warp_transform.apply(DVec2::new(x as f64, y as f64));
            output[(x, y)] = kernel_test_support::interpolate(
                input,
                Vec2::new(src.x as f32, src.y as f32),
                params,
            );
        }
    }
}

#[test]
fn test_generic_stepping_bicubic_matches_per_pixel() {
    // Bicubic with a similarity transform (translation + rotation + scale).
    // Stepped output should match per-pixel reference exactly (same f32 path).
    let width = 64;
    let height = 64;
    let input: Vec<f32> = (0..width * height)
        .map(|i| ((i as f32 * 0.037).sin() + 1.0) * 0.5)
        .collect();
    let input_buf = Buffer2::new(width, height, input);
    let transform = Transform::similarity(DVec2::new(3.0, 2.0), 0.05, 1.02);
    let wt = WarpTransform::new(transform);
    assert!(wt.is_linear());
    let params = config::test_support::warp_params(InterpolationMethod::Bicubic);

    let mut output_stepped = Buffer2::new_default(width, height);
    let mut output_reference = Buffer2::new_default(width, height);
    plane::warp(&input_buf, &mut output_stepped, &wt, &params);
    warp_per_pixel_reference(&input_buf, &mut output_reference, &wt, &params);

    for y in 0..height {
        for x in 0..width {
            let stepped = output_stepped[(x, y)];
            let reference = output_reference[(x, y)];
            assert!(
                (stepped - reference).abs() < 1e-4,
                "Bicubic mismatch at ({x}, {y}): stepped={stepped}, reference={reference}"
            );
        }
    }
}

#[test]
fn test_generic_stepping_lanczos2_matches_per_pixel() {
    let width = 64;
    let height = 64;
    let input: Vec<f32> = (0..width * height)
        .map(|i| ((i as f32 * 0.023).sin() + 1.0) * 0.5)
        .collect();
    let input_buf = Buffer2::new(width, height, input);
    let transform = Transform::similarity(DVec2::new(5.0, -1.0), 0.03, 0.98);
    let wt = WarpTransform::new(transform);
    assert!(wt.is_linear());
    let params = config::test_support::warp_params(InterpolationMethod::Lanczos2);

    let mut output_stepped = Buffer2::new_default(width, height);
    let mut output_reference = Buffer2::new_default(width, height);
    plane::warp(&input_buf, &mut output_stepped, &wt, &params);
    warp_per_pixel_reference(&input_buf, &mut output_reference, &wt, &params);

    for y in 0..height {
        for x in 0..width {
            let stepped = output_stepped[(x, y)];
            let reference = output_reference[(x, y)];
            assert!(
                (stepped - reference).abs() < 1e-4,
                "Lanczos2 mismatch at ({x}, {y}): stepped={stepped}, reference={reference}"
            );
        }
    }
}

#[test]
fn test_generic_stepping_lanczos4_matches_per_pixel() {
    let width = 64;
    let height = 64;
    let input: Vec<f32> = (0..width * height)
        .map(|i| ((i as f32 * 0.041).sin() + 1.0) * 0.5)
        .collect();
    let input_buf = Buffer2::new(width, height, input);
    let transform = Transform::similarity(DVec2::new(2.0, 3.0), -0.02, 1.01);
    let wt = WarpTransform::new(transform);
    assert!(wt.is_linear());
    let params = config::test_support::warp_params(InterpolationMethod::Lanczos4);

    let mut output_stepped = Buffer2::new_default(width, height);
    let mut output_reference = Buffer2::new_default(width, height);
    plane::warp(&input_buf, &mut output_stepped, &wt, &params);
    warp_per_pixel_reference(&input_buf, &mut output_reference, &wt, &params);

    for y in 0..height {
        for x in 0..width {
            let stepped = output_stepped[(x, y)];
            let reference = output_reference[(x, y)];
            assert!(
                (stepped - reference).abs() < 1e-4,
                "Lanczos4 mismatch at ({x}, {y}): stepped={stepped}, reference={reference}"
            );
        }
    }
}

#[test]
fn test_generic_stepping_nearest_matches_per_pixel() {
    let width = 64;
    let height = 64;
    let input: Vec<f32> = (0..width * height)
        .map(|i| ((i as f32 * 0.013).sin() + 1.0) * 0.5)
        .collect();
    let input_buf = Buffer2::new(width, height, input);
    let transform = Transform::similarity(DVec2::new(1.0, 2.0), 0.01, 1.0);
    let wt = WarpTransform::new(transform);
    assert!(wt.is_linear());
    let params = config::test_support::warp_params(InterpolationMethod::Nearest);

    let mut output_stepped = Buffer2::new_default(width, height);
    let mut output_reference = Buffer2::new_default(width, height);
    plane::warp(&input_buf, &mut output_stepped, &wt, &params);
    warp_per_pixel_reference(&input_buf, &mut output_reference, &wt, &params);

    for y in 0..height {
        for x in 0..width {
            let stepped = output_stepped[(x, y)];
            let reference = output_reference[(x, y)];
            // Nearest is exact — no floating point interpolation
            assert!(
                (stepped - reference).abs() < 1e-6,
                "Nearest mismatch at ({x}, {y}): stepped={stepped}, reference={reference}"
            );
        }
    }
}

#[test]
fn test_generic_stepping_disabled_for_homography() {
    // With a homography, is_linear() returns false, so stepping is disabled.
    // Output should match per-pixel reference exactly.
    let width = 32;
    let height = 32;
    let input: Vec<f32> = (0..width * height)
        .map(|i| ((i as f32 * 0.029).sin() + 1.0) * 0.5)
        .collect();
    let input_buf = Buffer2::new(width, height, input);
    // Homography with small perspective component
    let transform = Transform::homography([1.0, 0.0, 2.0, 0.0, 1.0, 1.0, 0.001, 0.0005]);
    let wt = WarpTransform::new(transform);
    assert!(!wt.is_linear());
    let params = config::test_support::warp_params(InterpolationMethod::Bicubic);

    let mut output_stepped = Buffer2::new_default(width, height);
    let mut output_reference = Buffer2::new_default(width, height);
    plane::warp(&input_buf, &mut output_stepped, &wt, &params);
    warp_per_pixel_reference(&input_buf, &mut output_reference, &wt, &params);

    for y in 0..height {
        for x in 0..width {
            let stepped = output_stepped[(x, y)];
            let reference = output_reference[(x, y)];
            assert!(
                (stepped - reference).abs() < 1e-6,
                "Homography mismatch at ({x}, {y}): stepped={stepped}, reference={reference}"
            );
        }
    }
}

#[test]
fn lanczos_homography_horizon_uses_border_and_zero_coverage() {
    const WIDTH: usize = 16;
    const HEIGHT: usize = 8;
    const HORIZON_X: usize = 8;
    const BORDER: f32 = -0.25;

    let input = Buffer2::new_filled(WIDTH, HEIGHT, 0.75);
    for horizon_scale in [1.0, 1.0 - 1e-12] {
        let transform = Transform::homography([
            1.0,
            0.0,
            0.0,
            0.0,
            1.0,
            0.0,
            -horizon_scale / HORIZON_X as f64,
            0.0,
        ]);
        let wt = WarpTransform::new(transform);
        let horizon = wt.apply(DVec2::new(HORIZON_X as f64, 0.0));
        if horizon_scale == 1.0 {
            assert!(!horizon.is_finite());
        } else {
            assert!(horizon.is_finite());
            assert!(horizon.x > i32::MAX as f64);
        }

        for method in [
            InterpolationMethod::Lanczos2,
            InterpolationMethod::Lanczos3,
            InterpolationMethod::Lanczos3,
            InterpolationMethod::Lanczos4,
        ] {
            let params = WarpParams {
                method,
                border_value: BORDER,
            };
            let mut output = Buffer2::new_default(WIDTH, HEIGHT);
            plane::warp(&input, &mut output, &wt, &params);
            let coverage = quality::maps(Vec2us::new(WIDTH, HEIGHT), &wt, method).coverage;

            for y in 0..HEIGHT {
                assert_eq!(
                    output[(HORIZON_X, y)],
                    BORDER,
                    "{method:?} horizon value at y={y}"
                );
                assert_eq!(
                    coverage[(HORIZON_X, y)],
                    0.0,
                    "{method:?} horizon coverage at y={y}"
                );
            }
            assert!(
                output.pixels().iter().all(|value| value.is_finite()),
                "{method:?} produced a non-finite value"
            );
            assert!(
                coverage.pixels().iter().all(|value| value.is_finite()),
                "{method:?} produced non-finite coverage"
            );
        }
    }
}

#[test]
fn warp_tiny_image_smaller_than_lanczos4_kernel() {
    let (w, h) = (3, 3);
    let input = Buffer2::new_filled(w, h, 0.5f32);
    let mut output = Buffer2::new_default(w, h);
    let wt = WarpTransform::new(Transform::identity());
    let params = config::test_support::warp_params(InterpolationMethod::Lanczos4);
    plane::warp(&input, &mut output, &wt, &params);
    for &value in output.pixels() {
        assert!(
            value.is_finite(),
            "tiny-image warp produced non-finite {value}"
        );
        assert!(
            (value - 0.5).abs() < 1e-4,
            "uniform image must stay 0.5, got {value}"
        );
    }
}
