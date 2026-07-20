use crate::stacking::registration::config::{self, InterpolationMethod};
use crate::stacking::registration::resample::{kernel, row};
use crate::stacking::registration::transform::{Transform, WarpTransform};
use crate::testing::synthetic::patterns;
use glam::{DVec2, Vec2};
use imaginarium::Buffer2;

/// Naive scalar Lanczos3 row warp used as reference for testing the optimized version.
fn lanczos_scalar(
    input: &Buffer2<f32>,
    output_row: &mut [f32],
    output_y: usize,
    wt: &WarpTransform,
) {
    let pixels = input.pixels();
    let input_width = input.width();
    let input_height = input.height();

    let y = output_y as f64;
    const A: usize = 3;

    for (x, out_pixel) in output_row.iter_mut().enumerate() {
        let src = wt.apply(DVec2::new(x as f64, y));
        let Some(pos) = row::source_position_in_footprint(src.x, src.y, input_width, input_height)
        else {
            *out_pixel = 0.0;
            continue;
        };
        let sx = pos.x;
        let sy = pos.y;

        let x0 = sx.floor() as i32;
        let y0 = sy.floor() as i32;
        let fx = sx - x0 as f32;
        let fy = sy - y0 as f32;
        let kx0 = x0 - 2;
        let ky0 = y0 - 2;

        if kx0 < 0 || ky0 < 0 || kx0 + 5 >= input_width as i32 || ky0 + 5 >= input_height as i32 {
            *out_pixel = kernel::bilinear_sample(input, Vec2::new(sx, sy), 0.0);
            continue;
        }

        let lut = kernel::get_lanczos_lut(A);

        let mut wx = [0.0f32; 6];
        for (i, w) in wx.iter_mut().enumerate() {
            let dx = fx - (i as i32 - 2) as f32;
            *w = lut.lookup(dx);
        }

        let mut wy = [0.0f32; 6];
        for (j, w) in wy.iter_mut().enumerate() {
            let dy = fy - (j as i32 - 2) as f32;
            *w = lut.lookup(dy);
        }

        let mut sum = 0.0f32;
        let mut w_in = 0.0f32;
        for (j, &wyj) in wy.iter().enumerate() {
            let py = y0 - 2 + j as i32;
            for (i, &wxi) in wx.iter().enumerate() {
                let px = x0 - 2 + i as i32;
                let weight = wxi * wyj;
                sum += pixels[py as usize * input_width + px as usize] * weight;
                w_in += weight;
            }
        }

        *out_pixel = if w_in.abs() < 1e-10 { 0.0 } else { sum / w_in };
    }
}

#[test]
fn test_bilinear_identity() {
    let width = 100;
    let height = 100;
    let input = patterns::diagonal_gradient(width, height);
    let identity = WarpTransform::new(Transform::identity());

    let mut output_row = vec![0.0f32; width];
    let y = 50;

    row::bilinear(&input, &mut output_row, y, &identity, 0.0);

    // With identity transform, output should match input
    for x in 1..width - 1 {
        let expected = input[(x, y)];
        assert!(
            (output_row[x] - expected).abs() < 0.01,
            "Mismatch at x={}: {} vs {}",
            x,
            output_row[x],
            expected
        );
    }
}

#[test]
fn test_bilinear_translation() {
    let width = 100;
    let height = 100;
    let input = patterns::diagonal_gradient(width, height);

    // Translate by (5, 3)
    let transform = Transform::translation(DVec2::new(5.0, 3.0));
    let inverse = WarpTransform::new(transform.inverse());

    let mut output_row = vec![0.0f32; width];
    let y = 50;

    row::bilinear(&input, &mut output_row, y, &inverse, 0.0);

    // Check that pixels are shifted
    for (x, &output_val) in output_row.iter().enumerate().skip(10).take(width - 20) {
        // Output at (x, y) should come from input at (x-5, y-3)
        let src_x = x as i32 - 5;
        let src_y = y as i32 - 3;
        if src_x >= 0 && src_y >= 0 {
            let expected = input[(src_x as usize, src_y as usize)];
            assert!(
                (output_val - expected).abs() < 0.01,
                "Mismatch at x={}: {} vs {}",
                x,
                output_val,
                expected
            );
        }
    }
}

#[test]
fn test_warp_row_simd_matches_scalar() {
    let width = 128;
    let height = 128;
    let input = patterns::diagonal_gradient(width, height);

    // Test with various transforms
    let transforms = vec![
        Transform::identity(),
        Transform::translation(DVec2::new(2.5, 1.7)),
        Transform::similarity(DVec2::new(3.0, 2.0), 0.1, 1.05),
    ];

    for transform in transforms {
        let inverse = WarpTransform::new(transform.inverse());

        for y in [0, 50, height - 1] {
            let mut output_simd = vec![0.0f32; width];
            let mut output_scalar = vec![0.0f32; width];

            row::bilinear(&input, &mut output_simd, y, &inverse, 0.0);
            row::bilinear_scalar(&input, &mut output_scalar, y, &inverse, 0.0);

            for x in 0..width {
                // Tolerance slightly relaxed for SIMD/scalar differences due to
                // different operation ordering and floating point precision
                assert!(
                    (output_simd[x] - output_scalar[x]).abs() < 1e-4,
                    "Row {}, x={}: SIMD {} vs Scalar {}",
                    y,
                    x,
                    output_simd[x],
                    output_scalar[x]
                );
            }
        }
    }
}

#[test]
fn test_warp_row_various_sizes() {
    let height = 64;
    let input_base = patterns::diagonal_gradient(256, height);

    // Test various widths including non-SIMD-aligned sizes
    for width in [
        1, 2, 3, 4, 5, 7, 8, 9, 15, 16, 17, 31, 32, 33, 63, 64, 100, 128,
    ] {
        let input = Buffer2::new(
            width,
            height,
            input_base
                .pixels()
                .iter()
                .take(width * height)
                .copied()
                .collect(),
        );
        let identity = WarpTransform::new(Transform::identity());

        let mut output_simd = vec![0.0f32; width];
        let mut output_scalar = vec![0.0f32; width];
        let y = height / 2;

        row::bilinear(&input, &mut output_simd, y, &identity, 0.0);
        row::bilinear_scalar(&input, &mut output_scalar, y, &identity, 0.0);

        for x in 0..width {
            assert!(
                (output_simd[x] - output_scalar[x]).abs() < 1e-5,
                "Width {}, x={}: SIMD {} vs Scalar {}",
                width,
                x,
                output_simd[x],
                output_scalar[x]
            );
        }
    }
}

#[test]
fn test_lanczos_scalar_identity() {
    let width = 100;
    let height = 100;
    let input = patterns::diagonal_gradient(width, height);
    let identity = WarpTransform::new(Transform::identity());

    let mut output_row = vec![0.0f32; width];
    let y = 50;

    lanczos_scalar(&input, &mut output_row, y, &identity);

    // With identity transform, output should match input (within Lanczos ringing tolerance)
    for x in 3..width - 3 {
        let expected = input[(x, y)];
        assert!(
            (output_row[x] - expected).abs() < 0.02,
            "Mismatch at x={}: {} vs {}",
            x,
            output_row[x],
            expected
        );
    }
}

#[test]
fn test_lanczos_scalar_various_sizes_match_optimized() {
    // Verify scalar and optimized Lanczos3 produce identical results across widths.
    // This replaces a weaker test that only checked is_finite().
    let height = 64;
    let input_base = patterns::diagonal_gradient(256, height);
    let params = config::test_support::warp_params(InterpolationMethod::Lanczos3);

    for width in [1, 2, 3, 4, 5, 7, 8, 16, 33, 64, 100] {
        let input = Buffer2::new(
            width,
            height,
            input_base
                .pixels()
                .iter()
                .take(width * height)
                .copied()
                .collect(),
        );
        let transform = Transform::translation(DVec2::new(1.5, 0.5));
        let inverse = WarpTransform::new(transform.inverse());

        let mut output_scalar = vec![0.0f32; width];
        let mut output_fast = vec![0.0f32; width];
        let y = height / 2;

        lanczos_scalar(&input, &mut output_scalar, y, &inverse);
        row::lanczos(&input, &mut output_fast, y, &inverse, &params);

        for x in 0..width {
            assert!(
                (output_fast[x] - output_scalar[x]).abs() < 1e-4,
                "Width {width}, x={x}: fast {} vs scalar {}",
                output_fast[x],
                output_scalar[x]
            );
        }
    }
}

#[test]
fn test_lanczos_matches_scalar() {
    let width = 128;
    let height = 128;
    let input = patterns::diagonal_gradient(width, height);
    // Disable clamping to match unclamped scalar reference
    let params = config::test_support::warp_params(InterpolationMethod::Lanczos3);

    let transforms = vec![
        Transform::identity(),
        Transform::translation(DVec2::new(2.5, 1.7)),
        Transform::similarity(DVec2::new(3.0, 2.0), 0.1, 1.05),
    ];

    for transform in transforms {
        let inverse = WarpTransform::new(transform.inverse());

        for y in [0, 50, height - 1] {
            let mut output_fast = vec![0.0f32; width];
            let mut output_scalar = vec![0.0f32; width];

            row::lanczos(&input, &mut output_fast, y, &inverse, &params);
            lanczos_scalar(&input, &mut output_scalar, y, &inverse);

            for x in 0..width {
                assert!(
                    (output_fast[x] - output_scalar[x]).abs() < 1e-4,
                    "Row {y}, x={x}: fast {} vs scalar {}",
                    output_fast[x],
                    output_scalar[x]
                );
            }
        }
    }
}

#[test]
fn test_lanczos_preserves_signed_constants_at_interior_and_edges() {
    let (width, height) = (24, 20);
    let identity = WarpTransform::new(Transform::identity());

    for method in [
        InterpolationMethod::Lanczos2,
        InterpolationMethod::Lanczos3,
        InterpolationMethod::Lanczos4,
    ] {
        let params = config::test_support::warp_params(method);
        for expected in [-1.25, 0.0, 2.5] {
            let input = patterns::uniform(width, height, expected);
            for y in [0, 1, 4, 10, height - 1] {
                let mut output = vec![0.0; width];
                row::lanczos(&input, &mut output, y, &identity, &params);
                for (x, actual) in output.into_iter().enumerate() {
                    assert!(
                        (actual - expected).abs() < 2e-5,
                        "{method:?} ({x}, {y}): expected {expected}, got {actual}"
                    );
                }
            }
        }
    }
}

#[test]
fn test_lanczos_is_translation_invariant_for_signed_data() {
    let (width, height) = (24, 20);
    let input = Buffer2::new(
        width,
        height,
        (0..width * height)
            .map(|i| ((i * 13 + i / width * 7) % 31) as f32 / 9.0 - 1.7)
            .collect(),
    );
    let offset = 2.25;
    let shifted = Buffer2::new(
        width,
        height,
        input.pixels().iter().map(|value| value + offset).collect(),
    );
    let inverse = WarpTransform::new(Transform::translation(DVec2::new(0.37, -0.43)).inverse());

    for method in [
        InterpolationMethod::Lanczos2,
        InterpolationMethod::Lanczos3,
        InterpolationMethod::Lanczos4,
    ] {
        let params = config::test_support::warp_params(method);
        for y in [0, 1, 5, 10, height - 1] {
            let mut output = vec![0.0; width];
            let mut shifted_output = vec![0.0; width];
            row::lanczos(&input, &mut output, y, &inverse, &params);
            row::lanczos(&shifted, &mut shifted_output, y, &inverse, &params);
            for x in 1..width {
                let actual_offset = shifted_output[x] - output[x];
                assert!(
                    (actual_offset - offset).abs() < 5e-5,
                    "{method:?} ({x}, {y}): expected offset {offset}, got {actual_offset}"
                );
            }
        }
    }
}

#[test]
fn test_lanczos_various_sizes() {
    let height = 64;
    let input_base = patterns::diagonal_gradient(256, height);
    // Disable clamping to match unclamped scalar reference
    let params = config::test_support::warp_params(InterpolationMethod::Lanczos3);

    for width in [1, 2, 3, 7, 8, 16, 33, 64, 100] {
        let input = Buffer2::new(
            width,
            height,
            input_base
                .pixels()
                .iter()
                .take(width * height)
                .copied()
                .collect(),
        );
        let transform = Transform::translation(DVec2::new(1.5, 0.5));
        let inverse = WarpTransform::new(transform.inverse());

        let mut output_fast = vec![0.0f32; width];
        let mut output_scalar = vec![0.0f32; width];
        let y = height / 2;

        row::lanczos(&input, &mut output_fast, y, &inverse, &params);
        lanczos_scalar(&input, &mut output_scalar, y, &inverse);

        for x in 0..width {
            assert!(
                (output_fast[x] - output_scalar[x]).abs() < 1e-4,
                "Width {width}, x={x}: fast {} vs scalar {}",
                output_fast[x],
                output_scalar[x]
            );
        }
    }
}

#[test]
fn test_bilinear_sample_hand_computed() {
    // 3x3 image:
    //   0  1  2
    //   3  4  5
    //   6  7  8
    let data = vec![0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
    let input = Buffer2::new(3, 3, data);

    // At integer pixel (1, 1): exactly 4.0
    assert!(
        (kernel::bilinear_sample(&input, Vec2::new(1.0, 1.0), 0.0) - 4.0).abs() < 1e-6,
        "At (1,1): expected 4.0, got {}",
        kernel::bilinear_sample(&input, Vec2::new(1.0, 1.0), 0.0)
    );

    // At (0.5, 0.5): bilinear of [0,1,3,4]
    // x0=0, y0=0, fx=0.5, fy=0.5
    // p00=0, p10=1, p01=3, p11=4
    // top = 0 + 0.5*(1-0) = 0.5
    // bottom = 3 + 0.5*(4-3) = 3.5
    // result = 0.5 + 0.5*(3.5 - 0.5) = 0.5 + 1.5 = 2.0
    assert!(
        (kernel::bilinear_sample(&input, Vec2::new(0.5, 0.5), 0.0) - 2.0).abs() < 1e-6,
        "At (0.5, 0.5): expected 2.0, got {}",
        kernel::bilinear_sample(&input, Vec2::new(0.5, 0.5), 0.0)
    );

    // At (1.5, 0.5): bilinear of [1,2,4,5]
    // x0=1, y0=0, fx=0.5, fy=0.5
    // p00=1, p10=2, p01=4, p11=5
    // top = 1 + 0.5*(2-1) = 1.5
    // bottom = 4 + 0.5*(5-4) = 4.5
    // result = 1.5 + 0.5*(4.5 - 1.5) = 1.5 + 1.5 = 3.0
    assert!(
        (kernel::bilinear_sample(&input, Vec2::new(1.5, 0.5), 0.0) - 3.0).abs() < 1e-6,
        "At (1.5, 0.5): expected 3.0, got {}",
        kernel::bilinear_sample(&input, Vec2::new(1.5, 0.5), 0.0)
    );

    // At (0.25, 0.75): bilinear of [0,1,3,4]
    // x0=0, y0=0, fx=0.25, fy=0.75
    // top = 0 + 0.25*(1-0) = 0.25
    // bottom = 3 + 0.25*(4-3) = 3.25
    // result = 0.25 + 0.75*(3.25-0.25) = 0.25 + 2.25 = 2.5
    assert!(
        (kernel::bilinear_sample(&input, Vec2::new(0.25, 0.75), 0.0) - 2.5).abs() < 1e-6,
        "At (0.25, 0.75): expected 2.5, got {}",
        kernel::bilinear_sample(&input, Vec2::new(0.25, 0.75), 0.0)
    );
}

#[test]
fn test_bilinear_sample_border_value() {
    // 2x2 image: [[10, 20], [30, 40]]
    let input = Buffer2::new(2, 2, vec![10.0, 20.0, 30.0, 40.0]);

    // Sampling outside uses border_value
    // At (-1.0, 0.0): x0 = floor(-1.0) = -1, y0 = 0
    // All four neighbors involve x=-1 or x=0
    // p00 = sample(-1, 0) = border = -5.0
    // p10 = sample(0, 0) = 10.0
    // p01 = sample(-1, 1) = border = -5.0
    // p11 = sample(0, 1) = 30.0
    // fx = -1.0 - (-1) = 0.0, fy = 0.0
    // top = -5.0 + 0.0*(10.0 - (-5.0)) = -5.0
    // bottom = -5.0 + 0.0*(30.0 - (-5.0)) = -5.0
    // result = -5.0 + 0.0*(...) = -5.0
    assert!(
        (kernel::bilinear_sample(&input, Vec2::new(-1.0, 0.0), -5.0) - (-5.0)).abs() < 1e-6,
        "At (-1.0, 0.0): expected -5.0, got {}",
        kernel::bilinear_sample(&input, Vec2::new(-1.0, 0.0), -5.0)
    );
}

/// Naive scalar Lanczos row warp for arbitrary `a`, used as reference.
fn lanczos_scalar_ref(
    input: &Buffer2<f32>,
    output_row: &mut [f32],
    output_y: usize,
    wt: &WarpTransform,
    a: usize,
) {
    let pixels = input.pixels();
    let input_width = input.width();
    let input_height = input.height();
    let y = output_y as f64;
    let size = 2 * a;
    let a_i32 = a as i32;
    let lut = kernel::get_lanczos_lut(a);

    for (x, out_pixel) in output_row.iter_mut().enumerate() {
        let src = wt.apply(DVec2::new(x as f64, y));
        let Some(pos) = row::source_position_in_footprint(src.x, src.y, input_width, input_height)
        else {
            *out_pixel = 0.0;
            continue;
        };
        let sx = pos.x;
        let sy = pos.y;
        let x0 = sx.floor() as i32;
        let y0 = sy.floor() as i32;
        let fx = sx - x0 as f32;
        let fy = sy - y0 as f32;
        let kx0 = x0 - a_i32 + 1;
        let ky0 = y0 - a_i32 + 1;

        if kx0 < 0
            || ky0 < 0
            || kx0 + size as i32 > input_width as i32
            || ky0 + size as i32 > input_height as i32
        {
            *out_pixel = kernel::bilinear_sample(input, pos, 0.0);
            continue;
        }

        let mut wx = vec![0.0f32; size];
        for (i, w) in wx.iter_mut().enumerate() {
            *w = lut.lookup(fx - (i as i32 - a_i32 + 1) as f32);
        }
        let mut wy = vec![0.0f32; size];
        for (j, w) in wy.iter_mut().enumerate() {
            *w = lut.lookup(fy - (j as i32 - a_i32 + 1) as f32);
        }
        let mut sum = 0.0f32;
        let mut w_in = 0.0f32;
        for (j, &wyj) in wy.iter().enumerate() {
            let py = y0 - a_i32 + 1 + j as i32;
            for (i, &wxi) in wx.iter().enumerate() {
                let px = x0 - a_i32 + 1 + i as i32;
                let weight = wxi * wyj;
                sum += pixels[py as usize * input_width + px as usize] * weight;
                w_in += weight;
            }
        }
        *out_pixel = if w_in.abs() < 1e-10 { 0.0 } else { sum / w_in };
    }
}

#[test]
fn test_lanczos2_matches_scalar_reference() {
    let width = 128;
    let height = 128;
    let input = patterns::diagonal_gradient(width, height);
    let params = config::test_support::warp_params(InterpolationMethod::Lanczos2);

    for transform in [
        Transform::identity(),
        Transform::translation(DVec2::new(2.5, 1.7)),
        Transform::similarity(DVec2::new(3.0, 2.0), 0.1, 1.05),
    ] {
        let inverse = WarpTransform::new(transform.inverse());
        for y in [0, 50, height - 1] {
            let mut output_fast = vec![0.0f32; width];
            let mut output_scalar = vec![0.0f32; width];
            row::lanczos(&input, &mut output_fast, y, &inverse, &params);
            lanczos_scalar_ref(&input, &mut output_scalar, y, &inverse, 2);
            for x in 0..width {
                assert!(
                    (output_fast[x] - output_scalar[x]).abs() < 1e-4,
                    "L2 row {y}, x={x}: fast {} vs scalar {}",
                    output_fast[x],
                    output_scalar[x]
                );
            }
        }
    }
}

#[test]
fn test_lanczos4_matches_scalar_reference() {
    let width = 128;
    let height = 128;
    let input = patterns::diagonal_gradient(width, height);
    let params = config::test_support::warp_params(InterpolationMethod::Lanczos4);

    for transform in [
        Transform::identity(),
        Transform::translation(DVec2::new(2.5, 1.7)),
        Transform::similarity(DVec2::new(3.0, 2.0), 0.1, 1.05),
    ] {
        let inverse = WarpTransform::new(transform.inverse());
        for y in [0, 50, height - 1] {
            let mut output_fast = vec![0.0f32; width];
            let mut output_scalar = vec![0.0f32; width];
            row::lanczos(&input, &mut output_fast, y, &inverse, &params);
            lanczos_scalar_ref(&input, &mut output_scalar, y, &inverse, 4);
            for x in 0..width {
                assert!(
                    (output_fast[x] - output_scalar[x]).abs() < 1e-4,
                    "L4 row {y}, x={x}: fast {} vs scalar {}",
                    output_fast[x],
                    output_scalar[x]
                );
            }
        }
    }
}

#[test]
fn test_lanczos2_various_sizes() {
    let height = 64;
    let input_base = patterns::diagonal_gradient(256, height);
    let params = config::test_support::warp_params(InterpolationMethod::Lanczos2);

    for width in [1, 2, 3, 7, 8, 16, 33, 64, 100] {
        let input = Buffer2::new(
            width,
            height,
            input_base
                .pixels()
                .iter()
                .take(width * height)
                .copied()
                .collect(),
        );
        let transform = Transform::translation(DVec2::new(1.5, 0.5));
        let inverse = WarpTransform::new(transform.inverse());

        let mut output_fast = vec![0.0f32; width];
        let mut output_scalar = vec![0.0f32; width];
        let y = height / 2;

        row::lanczos(&input, &mut output_fast, y, &inverse, &params);
        lanczos_scalar_ref(&input, &mut output_scalar, y, &inverse, 2);

        for x in 0..width {
            assert!(
                (output_fast[x] - output_scalar[x]).abs() < 1e-4,
                "L2 width {width}, x={x}: fast {} vs scalar {}",
                output_fast[x],
                output_scalar[x]
            );
        }
    }
}

#[test]
fn test_lanczos4_various_sizes() {
    let height = 64;
    let input_base = patterns::diagonal_gradient(256, height);
    let params = config::test_support::warp_params(InterpolationMethod::Lanczos4);

    for width in [1, 2, 3, 7, 8, 16, 33, 64, 100] {
        let input = Buffer2::new(
            width,
            height,
            input_base
                .pixels()
                .iter()
                .take(width * height)
                .copied()
                .collect(),
        );
        let transform = Transform::translation(DVec2::new(1.5, 0.5));
        let inverse = WarpTransform::new(transform.inverse());

        let mut output_fast = vec![0.0f32; width];
        let mut output_scalar = vec![0.0f32; width];
        let y = height / 2;

        row::lanczos(&input, &mut output_fast, y, &inverse, &params);
        lanczos_scalar_ref(&input, &mut output_scalar, y, &inverse, 4);

        for x in 0..width {
            assert!(
                (output_fast[x] - output_scalar[x]).abs() < 1e-4,
                "L4 width {width}, x={x}: fast {} vs scalar {}",
                output_fast[x],
                output_scalar[x]
            );
        }
    }
}
