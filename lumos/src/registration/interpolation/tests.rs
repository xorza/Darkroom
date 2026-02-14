use std::f32::consts::PI;

use super::*;
use crate::common::Buffer2;
use crate::registration::transform::{Transform, WarpTransform};
use glam::DVec2;

/// Shorthand for tests: interpolate with a method and default border/clamp settings.
fn interp(data: &Buffer2<f32>, x: f32, y: f32, method: InterpolationMethod) -> f32 {
    interpolate(data, x, y, &WarpParams::new(method))
}

const TOL: f32 = 1e-5;

// ============================================================================
// Lanczos kernel and LUT tests
// ============================================================================

#[test]
fn test_lanczos_kernel_compute_at_zero() {
    // L(0, a) = 1.0 by definition (limit of sinc(x) * sinc(x/a) as x -> 0)
    assert!((lanczos_kernel_compute(0.0, 2.0) - 1.0).abs() < TOL);
    assert!((lanczos_kernel_compute(0.0, 3.0) - 1.0).abs() < TOL);
    assert!((lanczos_kernel_compute(0.0, 4.0) - 1.0).abs() < TOL);
}

#[test]
fn test_lanczos_kernel_compute_at_integers() {
    // sinc(n) = sin(n*pi) / (n*pi) = 0 for all nonzero integers
    // So L(n, a) = 0 for integer n != 0
    for a in [2.0, 3.0, 4.0] {
        for n in 1..=(a as i32 - 1) {
            let val = lanczos_kernel_compute(n as f32, a);
            assert!(val.abs() < TOL, "L({n}, {a}) should be 0, got {val}");
            let val_neg = lanczos_kernel_compute(-(n as f32), a);
            assert!(
                val_neg.abs() < TOL,
                "L({}, {a}) should be 0, got {val_neg}",
                -n
            );
        }
    }
}

#[test]
fn test_lanczos_kernel_compute_at_boundary() {
    // L(a, a) = 0 by definition (outside support)
    assert_eq!(lanczos_kernel_compute(3.0, 3.0), 0.0);
    assert_eq!(lanczos_kernel_compute(-3.0, 3.0), 0.0);
    assert_eq!(lanczos_kernel_compute(2.0, 2.0), 0.0);
    assert_eq!(lanczos_kernel_compute(4.0, 4.0), 0.0);
}

#[test]
fn test_lanczos_kernel_compute_outside_support() {
    assert_eq!(lanczos_kernel_compute(3.5, 3.0), 0.0);
    assert_eq!(lanczos_kernel_compute(-4.1, 3.0), 0.0);
    assert_eq!(lanczos_kernel_compute(100.0, 3.0), 0.0);
}

#[test]
fn test_lanczos_kernel_compute_at_half() {
    // L(0.5, 3) = sinc(0.5) * sinc(0.5/3) = [sin(pi/2)/(pi/2)] * [sin(pi/6)/(pi/6)]
    // sinc(0.5) = sin(pi*0.5) / (pi*0.5) = 1.0 / (pi/2) = 2/pi
    // sinc(1/6) = sin(pi/6) / (pi/6) = 0.5 / (pi/6) = 3/pi
    // L(0.5, 3) = (2/pi) * (3/pi) = 6 / pi^2
    let expected = 6.0 / (PI * PI);
    let actual = lanczos_kernel_compute(0.5, 3.0);
    assert!(
        (actual - expected).abs() < 1e-6,
        "L(0.5, 3) = 6/pi^2 = {expected}, got {actual}"
    );
}

#[test]
fn test_lanczos_kernel_compute_symmetry() {
    // L(x) = L(-x) for all x
    for &x in &[0.1, 0.5, 1.0, 1.5, 2.5] {
        let pos = lanczos_kernel_compute(x, 3.0);
        let neg = lanczos_kernel_compute(-x, 3.0);
        assert!(
            (pos - neg).abs() < TOL,
            "Symmetry broken: L({x}) = {pos}, L(-{x}) = {neg}"
        );
    }
}

#[test]
fn test_lanczos_lut_matches_direct_computation() {
    // LUT should match direct computation within quantization tolerance.
    // LUT resolution is 4096 samples/unit, so max quantization error ~0.5/4096 in x,
    // which maps to at most ~0.001 in kernel value.
    for a in [2, 3, 4] {
        let a_f32 = a as f32;
        let lut = get_lanczos_lut(a);

        for &x in &[0.0, 0.25, 0.5, 0.75, 1.0, 1.5, 2.0, 2.5] {
            if x >= a_f32 {
                continue;
            }
            let direct = lanczos_kernel_compute(x, a_f32);
            let lut_val = lut.lookup(x);
            assert!(
                (direct - lut_val).abs() < 0.001,
                "LUT mismatch at x={x}, a={a}: direct={direct}, lut={lut_val}"
            );
        }
    }
}

#[test]
fn test_lanczos_lut_symmetry() {
    for a in [2, 3, 4] {
        let lut = get_lanczos_lut(a);
        for &x in &[0.1, 0.5, 1.0, 1.5] {
            let pos = lut.lookup(x);
            let neg = lut.lookup(-x);
            assert!(
                (pos - neg).abs() < 1e-6,
                "LUT symmetry broken at x={x}, a={a}: +x={pos}, -x={neg}"
            );
        }
    }
}

#[test]
fn test_lanczos_lut_special_values() {
    for a in [2, 3, 4] {
        let lut = get_lanczos_lut(a);
        let a_f32 = a as f32;

        // At x=0: kernel = 1
        assert!(
            (lut.lookup(0.0) - 1.0).abs() < 0.001,
            "LUT(0) for a={a}: expected 1.0, got {}",
            lut.lookup(0.0)
        );

        // At integer positions (except 0): kernel ~ 0
        for i in 1..a {
            let val = lut.lookup(i as f32);
            assert!(
                val.abs() < 0.001,
                "LUT({i}) for a={a}: expected ~0, got {val}"
            );
        }

        // At/beyond boundary: kernel = 0
        assert_eq!(lut.lookup(a_f32 + 0.1), 0.0);
    }
}

// ============================================================================
// Bicubic kernel tests
// ============================================================================

#[test]
fn test_bicubic_kernel_exact_values() {
    // Catmull-Rom with a = -0.5:
    //   K(0) = 1
    //   K(0.5): inner branch, abs_x=0.5
    //     = ((-0.5+2)*0.5 - (-0.5+3)) * 0.5^2 + 1
    //     = (1.5*0.5 - 2.5) * 0.25 + 1
    //     = (0.75 - 2.5) * 0.25 + 1
    //     = -1.75 * 0.25 + 1
    //     = -0.4375 + 1 = 0.5625
    assert!((bicubic_kernel(0.0) - 1.0).abs() < TOL);
    assert!((bicubic_kernel(0.5) - 0.5625).abs() < TOL);
    assert!((bicubic_kernel(-0.5) - 0.5625).abs() < TOL);

    // K(1.0): inner branch boundary
    //   = (1.5*1.0 - 2.5) * 1.0 + 1 = -1.0 + 1 = 0
    assert!((bicubic_kernel(1.0)).abs() < TOL);

    // K(1.5): outer branch, abs_x=1.5, a=-0.5
    //   = ((-0.5)*1.5 - 5*(-0.5)) * 1.5 + 8*(-0.5)) * 1.5 - 4*(-0.5)
    //   = (-0.75 + 2.5) * 1.5 + (-4)) * 1.5 + 2
    //   = (1.75 * 1.5 - 4.0) * 1.5 + 2
    //   = (2.625 - 4.0) * 1.5 + 2
    //   = -1.375 * 1.5 + 2 = -2.0625 + 2 = -0.0625
    assert!(
        (bicubic_kernel(1.5) - (-0.0625)).abs() < TOL,
        "K(1.5) = {}, expected -0.0625",
        bicubic_kernel(1.5)
    );

    // K(2.0) = 0
    assert!(bicubic_kernel(2.0).abs() < TOL);
    assert!(bicubic_kernel(-2.0).abs() < TOL);

    // K(2.5) = 0 (outside support)
    assert_eq!(bicubic_kernel(2.5), 0.0);
    assert_eq!(bicubic_kernel(-3.0), 0.0);
}

#[test]
fn test_bicubic_kernel_continuity_at_one() {
    // The kernel should be continuous at |x|=1 (branch transition).
    let left = bicubic_kernel(1.0 - 1e-4);
    let right = bicubic_kernel(1.0 + 1e-4);
    assert!(
        (left - right).abs() < 0.01,
        "Discontinuity at x=1: left={left}, right={right}"
    );
}

// ============================================================================
// sample_pixel tests
// ============================================================================

#[test]
fn test_sample_pixel_in_bounds() {
    // 3x2 image: [[10, 20, 30], [40, 50, 60]]
    let data = [10.0, 20.0, 30.0, 40.0, 50.0, 60.0];
    assert_eq!(sample_pixel(&data, 3, 2, 0, 0, -1.0), 10.0);
    assert_eq!(sample_pixel(&data, 3, 2, 1, 0, -1.0), 20.0);
    assert_eq!(sample_pixel(&data, 3, 2, 2, 0, -1.0), 30.0);
    assert_eq!(sample_pixel(&data, 3, 2, 0, 1, -1.0), 40.0);
    assert_eq!(sample_pixel(&data, 3, 2, 2, 1, -1.0), 60.0);
}

#[test]
fn test_sample_pixel_out_of_bounds() {
    let data = [10.0, 20.0, 30.0, 40.0];
    let bv = -999.0;
    // Negative coordinates
    assert_eq!(sample_pixel(&data, 2, 2, -1, 0, bv), bv);
    assert_eq!(sample_pixel(&data, 2, 2, 0, -1, bv), bv);
    // Beyond width/height
    assert_eq!(sample_pixel(&data, 2, 2, 2, 0, bv), bv);
    assert_eq!(sample_pixel(&data, 2, 2, 0, 2, bv), bv);
    assert_eq!(sample_pixel(&data, 2, 2, 100, 100, bv), bv);
}

// ============================================================================
// Nearest interpolation tests
// ============================================================================

#[test]
fn test_nearest_interpolation() {
    // 2x2 image: [[0, 1], [2, 3]]
    let data_buf = Buffer2::new(2, 2, vec![0.0, 1.0, 2.0, 3.0]);

    // round(0.4) = 0, round(0.4) = 0 => pixel (0,0) = 0.0
    assert_eq!(
        interp(&data_buf, 0.4, 0.4, InterpolationMethod::Nearest),
        0.0
    );
    // round(1.4) = 1, round(0.4) = 0 => pixel (1,0) = 1.0
    assert_eq!(
        interp(&data_buf, 1.4, 0.4, InterpolationMethod::Nearest),
        1.0
    );
    // round(0.4) = 0, round(1.4) = 1 => pixel (0,1) = 2.0
    assert_eq!(
        interp(&data_buf, 0.4, 1.4, InterpolationMethod::Nearest),
        2.0
    );
    // round(1.4) = 1, round(1.4) = 1 => pixel (1,1) = 3.0
    assert_eq!(
        interp(&data_buf, 1.4, 1.4, InterpolationMethod::Nearest),
        3.0
    );
}

#[test]
fn test_nearest_at_half_rounds_away() {
    // 2x2 image: [[10, 20], [30, 40]]
    let data_buf = Buffer2::new(2, 2, vec![10.0, 20.0, 30.0, 40.0]);

    // At x=0.5: round(0.5) = 1 (f32::round rounds 0.5 away from zero)
    // So (0.5, 0.0) -> pixel (1, 0) = 20.0
    assert_eq!(
        interp(&data_buf, 0.5, 0.0, InterpolationMethod::Nearest),
        20.0
    );
}

// ============================================================================
// Bilinear interpolation tests with hand-computed values
// ============================================================================

#[test]
fn test_bilinear_hand_computed() {
    // 2x2 image: [[0, 2], [4, 6]]
    // Pixel layout: p(0,0)=0, p(1,0)=2, p(0,1)=4, p(1,1)=6
    let data_buf = Buffer2::new(2, 2, vec![0.0, 2.0, 4.0, 6.0]);

    // At (0.0, 0.0): exactly p(0,0) = 0
    assert!((interp(&data_buf, 0.0, 0.0, InterpolationMethod::Bilinear) - 0.0).abs() < TOL);

    // At (1.0, 0.0): exactly p(1,0) = 2
    assert!((interp(&data_buf, 1.0, 0.0, InterpolationMethod::Bilinear) - 2.0).abs() < TOL);

    // At (0.5, 0.0): top = 0 + 0.5*(2-0) = 1.0, bottom would need y1 (border=0)
    // x0=0, y0=0, fx=0.5, fy=0.0
    // p00=0, p10=2, p01=4, p11=6
    // top = 0 + 0.5*(2-0) = 1.0
    // bottom = 4 + 0.5*(6-4) = 5.0
    // result = 1.0 + 0.0*(5.0 - 1.0) = 1.0
    assert!((interp(&data_buf, 0.5, 0.0, InterpolationMethod::Bilinear) - 1.0).abs() < TOL);

    // At (0.5, 0.5):
    // top = 0 + 0.5*(2-0) = 1.0
    // bottom = 4 + 0.5*(6-4) = 5.0
    // result = 1.0 + 0.5*(5.0-1.0) = 3.0
    assert!((interp(&data_buf, 0.5, 0.5, InterpolationMethod::Bilinear) - 3.0).abs() < TOL);

    // At (0.25, 0.75):
    // x0=0, y0=0, fx=0.25, fy=0.75
    // top = 0 + 0.25*(2-0) = 0.5
    // bottom = 4 + 0.25*(6-4) = 4.5
    // result = 0.5 + 0.75*(4.5-0.5) = 0.5 + 3.0 = 3.5
    assert!((interp(&data_buf, 0.25, 0.75, InterpolationMethod::Bilinear) - 3.5).abs() < TOL);
}

#[test]
fn test_bilinear_uniform_image() {
    // A uniform image should interpolate to the same constant everywhere
    let data_buf = Buffer2::new(2, 2, vec![7.0, 7.0, 7.0, 7.0]);
    assert!((interp(&data_buf, 0.3, 0.7, InterpolationMethod::Bilinear) - 7.0).abs() < TOL);
    assert!((interp(&data_buf, 0.99, 0.01, InterpolationMethod::Bilinear) - 7.0).abs() < TOL);
}

// ============================================================================
// Bicubic interpolation tests
// ============================================================================

#[test]
fn test_bicubic_at_pixel_centers() {
    // At integer pixel positions, bicubic should exactly reproduce pixel values.
    // Using a 4x4 grid so interior pixels (1,1) and (2,2) have full support.
    let data: Vec<f32> = (0..16).map(|i| i as f32).collect();
    let data_buf = Buffer2::new(4, 4, data);

    // pixel (1,1) = 1*4+1 = 5
    assert!((interp(&data_buf, 1.0, 1.0, InterpolationMethod::Bicubic) - 5.0).abs() < 0.01);
    // pixel (2,2) = 2*4+2 = 10
    assert!((interp(&data_buf, 2.0, 2.0, InterpolationMethod::Bicubic) - 10.0).abs() < 0.01);
}

#[test]
fn test_bicubic_monotonicity_on_gradient() {
    // Row 1 of a 4x4: [4, 5, 6, 7] (horizontal gradient)
    // Sampling at y=1 between x=0.5 and x=2.5 should be monotonically increasing.
    let input: Vec<f32> = (0..16).map(|i| (i % 4) as f32).collect();
    let input_buf = Buffer2::new(4, 4, input);

    let v1 = interp(&input_buf, 0.5, 1.0, InterpolationMethod::Bicubic);
    let v2 = interp(&input_buf, 1.5, 1.0, InterpolationMethod::Bicubic);
    let v3 = interp(&input_buf, 2.5, 1.0, InterpolationMethod::Bicubic);
    assert!(v1 < v2, "v1={v1} should be < v2={v2}");
    assert!(v2 < v3, "v2={v2} should be < v3={v3}");
}

// ============================================================================
// Lanczos interpolation tests
// ============================================================================

#[test]
fn test_lanczos_at_pixel_centers() {
    // At exact pixel centers, Lanczos should reproduce pixel values.
    // 8x8 grid so interior pixels have full 6x6 support window.
    let data: Vec<f32> = (0..64).map(|i| i as f32).collect();
    let data_buf = Buffer2::new(8, 8, data);

    // pixel (3,3) = 3*8+3 = 27
    let val_3_3 = interp(
        &data_buf,
        3.0,
        3.0,
        InterpolationMethod::Lanczos3 { deringing: -1.0 },
    );
    assert!(
        (val_3_3 - 27.0).abs() < 0.05,
        "L3 at (3,3): expected 27.0, got {val_3_3}"
    );

    // pixel (4,4) = 4*8+4 = 36
    let val_4_4 = interp(
        &data_buf,
        4.0,
        4.0,
        InterpolationMethod::Lanczos3 { deringing: -1.0 },
    );
    assert!(
        (val_4_4 - 36.0).abs() < 0.05,
        "L3 at (4,4): expected 36.0, got {val_4_4}"
    );
}

#[test]
fn test_lanczos_preserves_dc() {
    // A uniform image should remain uniform after Lanczos interpolation.
    // This tests that weights sum to ~1 (partition of unity).
    let input_buf = Buffer2::new(8, 8, vec![0.5f32; 64]);

    let val1 = interp(
        &input_buf,
        3.3,
        4.7,
        InterpolationMethod::Lanczos3 { deringing: -1.0 },
    );
    let val2 = interp(
        &input_buf,
        2.1,
        5.9,
        InterpolationMethod::Lanczos3 { deringing: -1.0 },
    );

    assert!(
        (val1 - 0.5).abs() < 0.01,
        "DC preservation: expected 0.5, got {val1}"
    );
    assert!(
        (val2 - 0.5).abs() < 0.01,
        "DC preservation: expected 0.5, got {val2}"
    );
}

#[test]
fn test_lanczos2_vs_lanczos3_different_results() {
    // Lanczos2 (4x4 kernel) and Lanczos3 (6x6 kernel) should produce different
    // results on non-trivial data, since they use different kernel widths.
    let input: Vec<f32> = (0..64).map(|i| (i as f32).sin()).collect();
    let input_buf = Buffer2::new(8, 8, input);

    let v2 = interp(
        &input_buf,
        3.5,
        4.5,
        InterpolationMethod::Lanczos2 { deringing: -1.0 },
    );
    let v3 = interp(
        &input_buf,
        3.5,
        4.5,
        InterpolationMethod::Lanczos3 { deringing: -1.0 },
    );

    // They should be close but not identical
    assert!(
        (v2 - v3).abs() < 0.5,
        "L2 and L3 should be close: v2={v2}, v3={v3}"
    );
    assert!(
        (v2 - v3).abs() > 1e-6,
        "L2 and L3 should differ: v2={v2}, v3={v3}"
    );
}

// ============================================================================
// Different methods produce different results
// ============================================================================

#[test]
fn test_different_methods_produce_different_results() {
    // At a sub-pixel position on non-trivial data, all methods should give
    // slightly different results (different kernels = different weights).
    let data: Vec<f32> = (0..64).map(|i| (i as f32 * 0.1).sin() * 10.0).collect();
    let data_buf = Buffer2::new(8, 8, data);
    let x = 3.37;
    let y = 4.63;

    let nearest = interp(&data_buf, x, y, InterpolationMethod::Nearest);
    let bilinear = interp(&data_buf, x, y, InterpolationMethod::Bilinear);
    let bicubic = interp(&data_buf, x, y, InterpolationMethod::Bicubic);
    let lanczos3 = interp(
        &data_buf,
        x,
        y,
        InterpolationMethod::Lanczos3 { deringing: -1.0 },
    );

    // Nearest should differ from the rest (it picks one pixel)
    assert!(
        (nearest - bilinear).abs() > 0.01,
        "Nearest and Bilinear should differ"
    );
    // Bilinear and Bicubic should differ
    assert!(
        (bilinear - bicubic).abs() > 0.001,
        "Bilinear and Bicubic should differ: {bilinear} vs {bicubic}"
    );
    // Bicubic and Lanczos3 should differ
    assert!(
        (bicubic - lanczos3).abs() > 0.001,
        "Bicubic and Lanczos3 should differ: {bicubic} vs {lanczos3}"
    );
}

// ============================================================================
// Border handling
// ============================================================================

#[test]
fn test_border_value_returned_out_of_bounds() {
    let data_buf = Buffer2::new(2, 2, vec![1.0; 4]);

    // Inside: bilinear on uniform should give 1.0
    assert!((interp(&data_buf, 0.5, 0.5, InterpolationMethod::Bilinear) - 1.0).abs() < TOL);

    // Fully outside: default border_value is 0.0
    assert!((interp(&data_buf, -2.0, 0.0, InterpolationMethod::Bilinear) - 0.0).abs() < TOL);
    assert!((interp(&data_buf, 0.0, -2.0, InterpolationMethod::Bilinear) - 0.0).abs() < TOL);
}

#[test]
fn test_custom_border_value() {
    let data_buf = Buffer2::new(2, 2, vec![1.0; 4]);
    let params = WarpParams {
        method: InterpolationMethod::Bilinear,
        border_value: -99.0,
    };

    // Fully outside should use the custom border value
    let val = interpolate(&data_buf, -5.0, -5.0, &params);
    assert!(
        (val - (-99.0)).abs() < TOL,
        "Expected border_value -99.0, got {val}"
    );
}

// ============================================================================
// All methods exact at pixel centers
// ============================================================================

#[test]
fn test_all_methods_exact_at_pixel_centers() {
    let width = 16;
    let height = 16;
    // Use a pseudo-random-looking pattern
    let input: Vec<f32> = (0..width * height)
        .map(|i| ((i * 17) % 100) as f32 / 100.0)
        .collect();
    let input_buf = Buffer2::new(width, height, input.clone());

    let methods = [
        InterpolationMethod::Nearest,
        InterpolationMethod::Bilinear,
        InterpolationMethod::Bicubic,
        InterpolationMethod::Lanczos2 { deringing: -1.0 },
        InterpolationMethod::Lanczos3 { deringing: -1.0 },
        InterpolationMethod::Lanczos4 { deringing: -1.0 },
    ];

    for method in &methods {
        // Interior pixels only (enough kernel support)
        for y in 4..height - 4 {
            for x in 4..width - 4 {
                let expected = input[y * width + x];
                let sampled = interp(&input_buf, x as f32, y as f32, *method);
                let tolerance = if matches!(method, InterpolationMethod::Nearest) {
                    0.0
                } else {
                    0.01
                };
                assert!(
                    (sampled - expected).abs() <= tolerance,
                    "{method:?} at ({x}, {y}): expected {expected}, got {sampled}"
                );
            }
        }
    }
}

// ============================================================================
// Gradient preservation test
// ============================================================================

#[test]
fn test_interpolation_gradient_preservation() {
    // Horizontal gradient: pixel value = x / width
    let width = 64;
    let height = 64;
    let input: Vec<f32> = (0..width * height)
        .map(|i| (i % width) as f32 / width as f32)
        .collect();
    let input_buf = Buffer2::new(width, height, input);

    let methods = [
        InterpolationMethod::Bilinear,
        InterpolationMethod::Bicubic,
        InterpolationMethod::Lanczos3 { deringing: -1.0 },
    ];

    for method in &methods {
        // Sample at sub-pixel positions along the gradient
        let samples: Vec<f32> = (0..10)
            .map(|i| {
                let x = 10.0 + i as f32 * 0.5;
                interp(&input_buf, x, 32.0, *method)
            })
            .collect();

        // Should be monotonically increasing
        for i in 1..samples.len() {
            assert!(
                samples[i] >= samples[i - 1] - 0.001,
                "{method:?}: monotonicity broken at step {i}: {} < {}",
                samples[i],
                samples[i - 1]
            );
        }
    }
}

// ============================================================================
// Analytic function interpolation quality test
// ============================================================================

#[test]
fn test_bicubic_vs_lanczos_analytic_quality() {
    // Create image from analytic function: f(x,y) = sin(x/10) * cos(y/10)
    // Then sample at sub-pixel offsets and compare to the analytic ground truth.
    let width = 64;
    let height = 64;
    let input: Vec<f32> = (0..width * height)
        .map(|i| {
            let x = (i % width) as f32;
            let y = (i / width) as f32;
            (x / 10.0).sin() * (y / 10.0).cos()
        })
        .collect();
    let input_buf = Buffer2::new(width, height, input);

    let mut bicubic_error_sum = 0.0f32;
    let mut lanczos_error_sum = 0.0f32;
    let mut count = 0;

    for iy in 5..height - 5 {
        for ix in 5..width - 5 {
            let x = ix as f32 + 0.3;
            let y = iy as f32 + 0.7;

            let expected = (x / 10.0).sin() * (y / 10.0).cos();
            let bicubic_val = interp(&input_buf, x, y, InterpolationMethod::Bicubic);
            let lanczos_val = interp(
                &input_buf,
                x,
                y,
                InterpolationMethod::Lanczos3 { deringing: -1.0 },
            );

            bicubic_error_sum += (bicubic_val - expected).abs();
            lanczos_error_sum += (lanczos_val - expected).abs();
            count += 1;
        }
    }

    let bicubic_mae = bicubic_error_sum / count as f32;
    let lanczos_mae = lanczos_error_sum / count as f32;

    // Both should have very low error on smooth functions
    assert!(
        bicubic_mae < 0.005,
        "Bicubic MAE = {bicubic_mae}, expected < 0.005"
    );
    assert!(
        lanczos_mae < 0.005,
        "Lanczos MAE = {lanczos_mae}, expected < 0.005"
    );
}

// ============================================================================
// warp_image tests
// ============================================================================

#[test]
fn test_warp_identity_preserves_image() {
    let input: Vec<f32> = (0..16).map(|i| i as f32).collect();
    let input_buf = Buffer2::new(4, 4, input.clone());

    let mut output = Buffer2::new(4, 4, vec![0.0; 16]);
    warp_image(
        &input_buf,
        &mut output,
        &WarpTransform::new(Transform::identity()),
        &WarpParams::new(InterpolationMethod::Bilinear),
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

    let mut output = Buffer2::new(4, 4, vec![0.0; 16]);
    warp_image(
        &input_buf,
        &mut output,
        &WarpTransform::new(transform),
        &WarpParams::new(InterpolationMethod::Bilinear),
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

// ============================================================================
// warp_image with Lanczos3 (exercises the fast dispatch path)
// ============================================================================

#[test]
fn test_warp_image_lanczos3_identity() {
    let width = 32;
    let height = 32;
    let input: Vec<f32> = (0..width * height).map(|i| (i as f32) / 1024.0).collect();
    let input_buf = Buffer2::new(width, height, input);

    let mut output = Buffer2::new(width, height, vec![0.0; width * height]);
    warp_image(
        &input_buf,
        &mut output,
        &WarpTransform::new(Transform::identity()),
        &WarpParams::new(InterpolationMethod::Lanczos3 { deringing: 0.3 }),
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
fn test_warp_image_lanczos3_integer_translation() {
    let width = 64;
    let height = 64;
    let input: Vec<f32> = (0..width * height).map(|i| (i as f32) / 4096.0).collect();
    let input_buf = Buffer2::new(width, height, input);
    // output[p] = input[p + (5,3)]
    let transform = Transform::translation(DVec2::new(5.0, 3.0));

    let mut output = Buffer2::new(width, height, vec![0.0; width * height]);
    warp_image(
        &input_buf,
        &mut output,
        &WarpTransform::new(transform),
        &WarpParams::new(InterpolationMethod::Lanczos3 { deringing: 0.3 }),
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
fn test_warp_image_lanczos3_matches_per_pixel() {
    // Verify the optimized warp_image Lanczos3 path matches per-pixel interpolate().
    // Uses deringing=-1.0 (disabled) so the scalar interpolate() path is the reference.
    let width = 32;
    let height = 32;
    let input: Vec<f32> = (0..width * height)
        .map(|i| ((i as f32 * 0.037).sin() + 1.0) * 0.5)
        .collect();
    let input_buf = Buffer2::new(width, height, input);
    let transform = Transform::similarity(DVec2::new(16.0, 16.0), 0.03, 1.02);

    let mut output = Buffer2::new(width, height, vec![0.0; width * height]);
    let params = WarpParams::new(InterpolationMethod::Lanczos3 { deringing: -1.0 });
    warp_image(
        &input_buf,
        &mut output,
        &WarpTransform::new(transform),
        &params,
    );

    for y in 0..height {
        for x in 0..width {
            let src = transform.apply(DVec2::new(x as f64, y as f64));
            let expected = interpolate(&input_buf, src.x as f32, src.y as f32, &params);
            let actual = output[(x, y)];
            assert!(
                (actual - expected).abs() < 1e-4,
                "Mismatch at ({x}, {y}): warp_image={actual} vs interpolate={expected}"
            );
        }
    }
}

// ============================================================================
// WarpParams tests
// ============================================================================

#[test]
fn test_warp_params_default() {
    let p = WarpParams::default();
    assert_eq!(p.border_value, 0.0);
    assert_eq!(p.method, InterpolationMethod::default());
}

#[test]
fn test_warp_params_new() {
    let p = WarpParams::new(InterpolationMethod::Bicubic);
    assert_eq!(p.method, InterpolationMethod::Bicubic);
    assert_eq!(p.border_value, 0.0);
}
