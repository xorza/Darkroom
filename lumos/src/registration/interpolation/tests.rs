use super::*;
use crate::common::Buffer2;

const EPSILON: f32 = 1e-5;

// ============================================================================
// Lanczos LUT Accuracy Tests
// ============================================================================

#[test]
fn test_lanczos_lut_vs_direct_computation() {
    // Compare LUT results with direct computation across the entire kernel support
    for a in [2, 3, 4] {
        let a_f32 = a as f32;
        let lut = super::get_lanczos_lut(a);

        // Test many positions including edge cases
        let test_positions: Vec<f32> = (0..=1000)
            .map(|i| (i as f32 / 1000.0) * a_f32)
            .chain([-0.5, -1.0, -1.5, -2.0, -2.5, -3.0, -3.5, -4.0].into_iter())
            .collect();

        for x in test_positions {
            if x.abs() > a_f32 {
                continue;
            }

            let direct = super::lanczos_kernel_direct(x, a_f32);
            let lut_val = lut.lookup(x);

            // LUT with linear interpolation should be very accurate
            let diff = (direct - lut_val).abs();
            assert!(
                diff < 0.001,
                "LUT mismatch at x={}, a={}: direct={}, lut={}, diff={}",
                x,
                a,
                direct,
                lut_val,
                diff
            );
        }
    }
}

#[test]
fn test_lanczos_lut_symmetry() {
    // Verify the LUT preserves kernel symmetry
    for a in [2, 3, 4] {
        let lut = super::get_lanczos_lut(a);

        for i in 1..100 {
            let x = i as f32 / 50.0;
            let pos = lut.lookup(x);
            let neg = lut.lookup(-x);

            assert!(
                (pos - neg).abs() < 1e-6,
                "Symmetry broken at x={}, a={}: +x={}, -x={}",
                x,
                a,
                pos,
                neg
            );
        }
    }
}

#[test]
fn test_lanczos_lut_special_values() {
    for a in [2, 3, 4] {
        let lut = super::get_lanczos_lut(a);
        let a_f32 = a as f32;

        // At x=0, kernel should be 1
        assert!(
            (lut.lookup(0.0) - 1.0).abs() < 0.001,
            "LUT(0) should be 1.0 for a={}, got {}",
            a,
            lut.lookup(0.0)
        );

        // At integer positions (except 0), kernel should be ~0
        for i in 1..a {
            let val = lut.lookup(i as f32);
            assert!(
                val.abs() < 0.001,
                "LUT({}) should be ~0 for a={}, got {}",
                i,
                a,
                val
            );
        }

        // At boundary, kernel should be 0
        assert!(
            lut.lookup(a_f32).abs() < 1e-6,
            "LUT(a) should be 0 for a={}, got {}",
            a,
            lut.lookup(a_f32)
        );

        // Outside boundary, kernel should be 0
        assert_eq!(
            lut.lookup(a_f32 + 0.1),
            0.0,
            "LUT beyond boundary should be 0"
        );
    }
}

#[test]
fn test_lanczos_kernel_uses_lut() {
    // Verify that lanczos_kernel uses LUT for standard parameters
    // and produces identical results
    for a in [2.0f32, 3.0, 4.0] {
        for i in 0..100 {
            let x = (i as f32 - 50.0) / 20.0;
            let kernel_val = lanczos_kernel(x, a);
            let direct_val = super::lanczos_kernel_direct(x, a);

            // Should be very close (LUT with interpolation)
            let diff = (kernel_val - direct_val).abs();
            assert!(
                diff < 0.001,
                "Kernel mismatch at x={}, a={}: kernel={}, direct={}, diff={}",
                x,
                a,
                kernel_val,
                direct_val,
                diff
            );
        }
    }
}

#[test]
fn test_lanczos_kernel_fallback_for_nonstandard_a() {
    // For non-standard values of 'a', should fall back to direct computation
    let a = 2.5f32; // Not 2, 3, or 4
    let x = 0.5;

    let kernel_val = lanczos_kernel(x, a);
    let direct_val = super::lanczos_kernel_direct(x, a);

    // Should be exactly equal (using direct computation)
    assert_eq!(kernel_val, direct_val);
}

#[test]
fn test_lanczos_kernel_center() {
    // At center, kernel should be 1
    assert!((lanczos_kernel(0.0, 3.0) - 1.0).abs() < EPSILON);
}

#[test]
fn test_lanczos_kernel_zeros() {
    // At integer positions, sinc is 0 (except at 0)
    assert!(lanczos_kernel(1.0, 3.0).abs() < EPSILON);
    assert!(lanczos_kernel(2.0, 3.0).abs() < EPSILON);
    assert!(lanczos_kernel(-1.0, 3.0).abs() < EPSILON);
}

#[test]
fn test_lanczos_kernel_outside() {
    // Outside window, kernel is 0
    assert_eq!(lanczos_kernel(3.0, 3.0), 0.0);
    assert_eq!(lanczos_kernel(4.0, 3.0), 0.0);
    assert_eq!(lanczos_kernel(-3.0, 3.0), 0.0);
}

#[test]
fn test_lanczos_kernel_symmetry() {
    // Kernel should be symmetric
    assert!((lanczos_kernel(0.5, 3.0) - lanczos_kernel(-0.5, 3.0)).abs() < EPSILON);
    assert!((lanczos_kernel(1.5, 3.0) - lanczos_kernel(-1.5, 3.0)).abs() < EPSILON);
}

#[test]
fn test_bicubic_kernel_center() {
    assert!((bicubic_kernel(0.0) - 1.0).abs() < EPSILON);
}

#[test]
fn test_bicubic_kernel_edges() {
    assert!(bicubic_kernel(2.0).abs() < EPSILON);
    assert!(bicubic_kernel(-2.0).abs() < EPSILON);
}

#[test]
fn test_bicubic_kernel_continuity() {
    // Kernel should be continuous at x=1
    let left = bicubic_kernel(1.0 - 0.001);
    let right = bicubic_kernel(1.0 + 0.001);
    assert!((left - right).abs() < 0.01);
}

#[test]
fn test_nearest_interpolation() {
    let data = vec![0.0, 1.0, 2.0, 3.0];
    let data_buf = Buffer2::new(2, 2, data);
    let config = WarpConfig {
        method: InterpolationMethod::Nearest,
        ..Default::default()
    };

    // Center of pixels
    assert!((interpolate_pixel(&data_buf, 0.4, 0.4, &config) - 0.0).abs() < EPSILON);
    assert!((interpolate_pixel(&data_buf, 1.4, 0.4, &config) - 1.0).abs() < EPSILON);
    assert!((interpolate_pixel(&data_buf, 0.4, 1.4, &config) - 2.0).abs() < EPSILON);
    assert!((interpolate_pixel(&data_buf, 1.4, 1.4, &config) - 3.0).abs() < EPSILON);
}

#[test]
fn test_bilinear_center() {
    let data = vec![0.0, 2.0, 2.0, 4.0];
    let data_buf = Buffer2::new(2, 2, data);
    let config = WarpConfig {
        method: InterpolationMethod::Bilinear,
        ..Default::default()
    };

    // At pixel centers
    assert!((interpolate_pixel(&data_buf, 0.0, 0.0, &config) - 0.0).abs() < EPSILON);
    assert!((interpolate_pixel(&data_buf, 1.0, 0.0, &config) - 2.0).abs() < EPSILON);

    // Between pixels - should interpolate
    let center = interpolate_pixel(&data_buf, 0.5, 0.5, &config);
    assert!((center - 2.0).abs() < EPSILON); // Average of all 4
}

#[test]
fn test_bilinear_edge() {
    let data = vec![1.0, 1.0, 1.0, 1.0];
    let data_buf = Buffer2::new(2, 2, data);
    let config = WarpConfig {
        method: InterpolationMethod::Bilinear,
        ..Default::default()
    };

    // Uniform image should give same value everywhere
    assert!((interpolate_pixel(&data_buf, 0.3, 0.7, &config) - 1.0).abs() < EPSILON);
}

#[test]
fn test_bicubic_pixel_centers() {
    // At pixel centers, bicubic should return exact values
    let data: Vec<f32> = (0..16).map(|i| i as f32).collect();
    let data_buf = Buffer2::new(4, 4, data);
    let config = WarpConfig {
        method: InterpolationMethod::Bicubic,
        ..Default::default()
    };

    assert!((interpolate_pixel(&data_buf, 1.0, 1.0, &config) - 5.0).abs() < 0.01);
    assert!((interpolate_pixel(&data_buf, 2.0, 2.0, &config) - 10.0).abs() < 0.01);
}

#[test]
fn test_lanczos_pixel_centers() {
    // At pixel centers, Lanczos should return exact values
    let data: Vec<f32> = (0..64).map(|i| i as f32).collect();
    let data_buf = Buffer2::new(8, 8, data);
    let config = WarpConfig {
        method: InterpolationMethod::Lanczos3,
        normalize_kernel: true,
        ..Default::default()
    };

    assert!((interpolate_pixel(&data_buf, 3.0, 3.0, &config) - 27.0).abs() < 0.1);
    assert!((interpolate_pixel(&data_buf, 4.0, 4.0, &config) - 36.0).abs() < 0.1);
}

#[test]
fn test_border_handling() {
    let data = vec![1.0; 4];
    let data_buf = Buffer2::new(2, 2, data);
    let config = WarpConfig {
        method: InterpolationMethod::Bilinear,
        border_value: 0.0,
        ..Default::default()
    };

    // Inside - should interpolate to 1.0 since all pixels are 1.0
    assert!((interpolate_pixel(&data_buf, 0.5, 0.5, &config) - 1.0).abs() < EPSILON);

    // Fully outside - should return border value
    assert!((interpolate_pixel(&data_buf, -2.0, 0.0, &config) - 0.0).abs() < EPSILON);
}

#[test]
fn test_warp_identity() {
    let input: Vec<f32> = (0..16).map(|i| i as f32).collect();
    let input_buf = Buffer2::new(4, 4, input.clone());
    let transform = Transform::identity();
    let config = WarpConfig {
        method: InterpolationMethod::Bilinear,
        ..Default::default()
    };

    let output = warp_image(&input_buf, 4, 4, &transform, &config);

    // Identity transform should preserve the image at pixel centers
    for (i, (&inp, &out)) in input.iter().zip(output.iter()).enumerate() {
        assert!(
            (inp - out).abs() < 0.1,
            "Mismatch at pixel {}: {} vs {}",
            i,
            inp,
            out
        );
    }
}

#[test]
fn test_warp_translation() {
    // Create a simple image with a single bright pixel
    let mut input = vec![0.0f32; 16];
    input[5] = 1.0; // Position (1, 1)
    let input_buf = Buffer2::new(4, 4, input);

    // Translate by (1, 1)
    let transform = Transform::translation(1.0, 1.0);
    let config = WarpConfig {
        method: InterpolationMethod::Bilinear,
        ..Default::default()
    };

    let output = warp_image(&input_buf, 4, 4, &transform, &config);

    // The bright pixel should move to (2, 2)
    assert!(output[10] > 0.5, "Expected bright pixel at (2,2)");
    assert!(output[5] < 0.1, "Expected dark pixel at (1,1)");
}

#[test]
fn test_warp_scale() {
    // Create a 2x2 image
    let input = vec![1.0, 2.0, 3.0, 4.0];
    let input_buf = Buffer2::new(2, 2, input);

    // Scale 2x
    let transform = Transform::scale(2.0, 2.0);
    let config = WarpConfig {
        method: InterpolationMethod::Bilinear,
        ..Default::default()
    };

    let output = warp_image(&input_buf, 4, 4, &transform, &config);

    assert_eq!(output.len(), 16);
    // Top-left corner should still be ~1.0
    assert!((output[0] - 1.0).abs() < 0.1);
}

#[test]
fn test_interpolation_method_radius() {
    assert_eq!(InterpolationMethod::Nearest.kernel_radius(), 1);
    assert_eq!(InterpolationMethod::Bilinear.kernel_radius(), 1);
    assert_eq!(InterpolationMethod::Bicubic.kernel_radius(), 2);
    assert_eq!(InterpolationMethod::Lanczos2.kernel_radius(), 2);
    assert_eq!(InterpolationMethod::Lanczos3.kernel_radius(), 3);
    assert_eq!(InterpolationMethod::Lanczos4.kernel_radius(), 4);
}

#[test]
fn test_lanczos_preserves_dc() {
    // A uniform image should remain uniform after Lanczos interpolation
    let input = vec![0.5f32; 64];
    let input_buf = Buffer2::new(8, 8, input);
    let config = WarpConfig {
        method: InterpolationMethod::Lanczos3,
        normalize_kernel: true,
        ..Default::default()
    };

    // Sample at various sub-pixel positions
    let val1 = interpolate_pixel(&input_buf, 3.3, 4.7, &config);
    let val2 = interpolate_pixel(&input_buf, 2.1, 5.9, &config);

    assert!((val1 - 0.5).abs() < 0.01);
    assert!((val2 - 0.5).abs() < 0.01);
}

#[test]
fn test_warp_rotation() {
    // Create image with asymmetric pattern
    let mut input = vec![0.0f32; 64];
    input[0] = 1.0; // Top-left corner
    let input_buf = Buffer2::new(8, 8, input);

    // Rotate 90 degrees around center
    let transform = Transform::rotation_around(4.0, 4.0, std::f64::consts::FRAC_PI_2);
    let config = WarpConfig {
        method: InterpolationMethod::Bilinear,
        ..Default::default()
    };

    let output = warp_image(&input_buf, 8, 8, &transform, &config);

    // After 90 degree rotation, top-left should move
    // The bright pixel should be somewhere else
    assert!(
        output[0] < 0.5,
        "Top-left should not be bright after rotation"
    );
}

#[test]
fn test_bicubic_smooth_gradient() {
    // Bicubic should smoothly interpolate gradients
    let input: Vec<f32> = (0..16).map(|i| (i % 4) as f32).collect();
    let input_buf = Buffer2::new(4, 4, input);
    let config = WarpConfig {
        method: InterpolationMethod::Bicubic,
        ..Default::default()
    };

    // Sample between pixels
    let v1 = interpolate_pixel(&input_buf, 0.5, 1.0, &config);
    let v2 = interpolate_pixel(&input_buf, 1.5, 1.0, &config);
    let v3 = interpolate_pixel(&input_buf, 2.5, 1.0, &config);

    // Should be monotonically increasing in a gradient
    assert!(v1 < v2);
    assert!(v2 < v3);
}

#[test]
fn test_lanczos2_vs_lanczos3() {
    let input: Vec<f32> = (0..64).map(|i| (i as f32).sin()).collect();
    let input_buf = Buffer2::new(8, 8, input);

    let config2 = WarpConfig {
        method: InterpolationMethod::Lanczos2,
        ..Default::default()
    };
    let config3 = WarpConfig {
        method: InterpolationMethod::Lanczos3,
        ..Default::default()
    };

    let v2 = interpolate_pixel(&input_buf, 3.5, 4.5, &config2);
    let v3 = interpolate_pixel(&input_buf, 3.5, 4.5, &config3);

    // Both should give reasonable values (not wildly different)
    assert!((v2 - v3).abs() < 0.5);
}

#[test]
fn test_lanczos_clamping_prevents_overshoot() {
    // Create image with sharp edge (step function)
    // This is the classic case where Lanczos produces ringing artifacts
    let mut input = vec![0.0f32; 64];
    for y in 0..8 {
        for x in 4..8 {
            input[y * 8 + x] = 1.0;
        }
    }
    let input_buf = Buffer2::new(8, 8, input);

    let config_no_clamp = WarpConfig {
        method: InterpolationMethod::Lanczos3,
        clamp_output: false,
        ..Default::default()
    };

    let config_clamp = WarpConfig {
        method: InterpolationMethod::Lanczos3,
        clamp_output: true,
        ..Default::default()
    };

    // Sample near the sharp edge - Lanczos can overshoot here
    let val_no_clamp = interpolate_pixel(&input_buf, 3.7, 4.0, &config_no_clamp);
    let val_clamp = interpolate_pixel(&input_buf, 3.7, 4.0, &config_clamp);

    // Clamped value must be within [0, 1] range
    assert!(
        (0.0..=1.0).contains(&val_clamp),
        "Clamped value {} should be in [0, 1]",
        val_clamp
    );

    // Without clamping, Lanczos often produces slight overshoot/undershoot near edges
    // The clamped value should be different if there was overshoot
    // (Note: they may be equal if no overshoot occurred at this position)
    println!(
        "No clamp: {}, Clamp: {} (difference: {})",
        val_no_clamp,
        val_clamp,
        (val_no_clamp - val_clamp).abs()
    );
}

#[test]
fn test_lanczos_clamping_preserves_smooth_regions() {
    // In smooth regions, clamping should have minimal effect
    let input: Vec<f32> = (0..64).map(|i| (i as f32) / 64.0).collect();
    let input_buf = Buffer2::new(8, 8, input);

    let config_no_clamp = WarpConfig {
        method: InterpolationMethod::Lanczos3,
        clamp_output: false,
        ..Default::default()
    };

    let config_clamp = WarpConfig {
        method: InterpolationMethod::Lanczos3,
        clamp_output: true,
        ..Default::default()
    };

    // Sample in smooth gradient region
    let val_no_clamp = interpolate_pixel(&input_buf, 3.5, 4.5, &config_no_clamp);
    let val_clamp = interpolate_pixel(&input_buf, 3.5, 4.5, &config_clamp);

    // In smooth regions, clamping should have minimal effect
    assert!(
        (val_no_clamp - val_clamp).abs() < 0.01,
        "Clamping changed smooth region: {} vs {}",
        val_no_clamp,
        val_clamp
    );
}

// ============================================================================
// Milestone F: Test Hardening - Interpolation quality tests
// ============================================================================

/// Test that interpolation preserves gradients accurately
#[test]
fn test_interpolation_gradient_preservation() {
    // Create a linear gradient image
    let width = 64;
    let height = 64;
    let input: Vec<f32> = (0..width * height)
        .map(|i| (i % width) as f32 / width as f32)
        .collect();
    let input_buf = Buffer2::new(width, height, input);

    let methods = [
        InterpolationMethod::Bilinear,
        InterpolationMethod::Bicubic,
        InterpolationMethod::Lanczos3,
    ];

    for method in &methods {
        let config = WarpConfig {
            method: *method,
            ..Default::default()
        };

        // Sample at sub-pixel positions along the gradient
        let samples: Vec<f32> = (0..10)
            .map(|i| {
                let x = 10.0 + i as f32 * 0.5;
                interpolate_pixel(&input_buf, x, 32.0, &config)
            })
            .collect();

        // Check that samples are monotonically increasing (gradient preserved)
        for i in 1..samples.len() {
            assert!(
                samples[i] >= samples[i - 1] - 0.001,
                "{:?}: Gradient not preserved at step {}: {} < {}",
                method,
                i,
                samples[i],
                samples[i - 1]
            );
        }
    }
}

/// Test interpolation quality by comparing different methods on same input
#[test]
fn test_bicubic_vs_lanczos_quality() {
    // Create image with known analytic function: f(x,y) = sin(x/10) * cos(y/10)
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

    let config_bicubic = WarpConfig {
        method: InterpolationMethod::Bicubic,
        ..Default::default()
    };

    let config_lanczos = WarpConfig {
        method: InterpolationMethod::Lanczos3,
        ..Default::default()
    };

    // Sample at sub-pixel positions and compare to analytic function
    let mut bicubic_error_sum = 0.0f32;
    let mut lanczos_error_sum = 0.0f32;
    let mut count = 0;

    for iy in 5..height - 5 {
        for ix in 5..width - 5 {
            let x = ix as f32 + 0.3; // Sub-pixel offset
            let y = iy as f32 + 0.7;

            let expected = (x / 10.0).sin() * (y / 10.0).cos();
            let bicubic_val = interpolate_pixel(&input_buf, x, y, &config_bicubic);
            let lanczos_val = interpolate_pixel(&input_buf, x, y, &config_lanczos);

            bicubic_error_sum += (bicubic_val - expected).abs();
            lanczos_error_sum += (lanczos_val - expected).abs();
            count += 1;
        }
    }

    let bicubic_mae = bicubic_error_sum / count as f32;
    let lanczos_mae = lanczos_error_sum / count as f32;

    // Both should have low error
    assert!(bicubic_mae < 0.05, "Bicubic MAE too high: {}", bicubic_mae);
    assert!(lanczos_mae < 0.05, "Lanczos MAE too high: {}", lanczos_mae);

    println!(
        "Interpolation MAE comparison: Bicubic={:.6}, Lanczos={:.6}",
        bicubic_mae, lanczos_mae
    );
}

/// Test that bilinear interpolation is exact at pixel centers
#[test]
fn test_all_methods_exact_at_pixel_centers() {
    let width = 16;
    let height = 16;
    let input: Vec<f32> = (0..width * height)
        .map(|i| ((i * 17) % 100) as f32 / 100.0)
        .collect();
    let input_buf = Buffer2::new(width, height, input.clone());

    let methods = [
        InterpolationMethod::Nearest,
        InterpolationMethod::Bilinear,
        InterpolationMethod::Bicubic,
        InterpolationMethod::Lanczos2,
        InterpolationMethod::Lanczos3,
        InterpolationMethod::Lanczos4,
    ];

    for method in &methods {
        let config = WarpConfig {
            method: *method,
            ..Default::default()
        };

        // Sample at exact pixel centers
        for y in 2..height - 2 {
            for x in 2..width - 2 {
                let expected = input[y * width + x];
                let sampled = interpolate_pixel(&input_buf, x as f32, y as f32, &config);

                // Should be exact (or very close for Lanczos due to kernel shape)
                let tolerance = if matches!(method, InterpolationMethod::Nearest) {
                    0.0
                } else {
                    0.01
                };

                assert!(
                    (sampled - expected).abs() <= tolerance,
                    "{:?} at ({}, {}): expected {}, got {} (diff {})",
                    method,
                    x,
                    y,
                    expected,
                    sampled,
                    (sampled - expected).abs()
                );
            }
        }
    }
}

/// Test interpolation with extreme sub-pixel positions
#[test]
fn test_interpolation_extreme_subpixel() {
    let width = 32;
    let height = 32;
    let input: Vec<f32> = (0..width * height)
        .map(|i| i as f32 / (width * height) as f32)
        .collect();
    let input_buf = Buffer2::new(width, height, input);

    let config = WarpConfig {
        method: InterpolationMethod::Lanczos3,
        ..Default::default()
    };

    // Test positions very close to pixel boundaries
    let extreme_positions = [
        (10.0001, 10.0001),
        (10.9999, 10.9999),
        (10.5, 10.0001),
        (10.0001, 10.5),
    ];

    for &(x, y) in &extreme_positions {
        let val = interpolate_pixel(&input_buf, x, y, &config);

        // Should not produce NaN or infinity
        assert!(
            val.is_finite(),
            "Non-finite value at ({}, {}): {}",
            x,
            y,
            val
        );

        // Should be within reasonable range
        assert!(
            (-0.1..=1.1).contains(&val),
            "Value out of range at ({}, {}): {}",
            x,
            y,
            val
        );
    }
}
