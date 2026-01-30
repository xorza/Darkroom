//! Tests for SIMD convolution implementations.

use super::{convolve_row, convolve_row_scalar};

#[test]
fn test_convolve_row_scalar_identity() {
    let input = vec![1.0, 2.0, 3.0, 4.0, 5.0];
    let kernel = vec![0.0, 1.0, 0.0]; // Identity kernel
    let mut output = vec![0.0; 5];

    convolve_row_scalar(&input, &mut output, &kernel, 1);

    for i in 0..5 {
        assert!(
            (output[i] - input[i]).abs() < 1e-6,
            "Identity kernel should preserve values"
        );
    }
}

#[test]
fn test_convolve_row_scalar_average() {
    let input = vec![0.0, 0.0, 3.0, 0.0, 0.0];
    let kernel = vec![1.0 / 3.0, 1.0 / 3.0, 1.0 / 3.0]; // Average kernel
    let mut output = vec![0.0; 5];

    convolve_row_scalar(&input, &mut output, &kernel, 1);

    // Center pixel should be 1.0 (3.0 / 3.0)
    assert!((output[2] - 1.0).abs() < 1e-6);
    // Neighbors should be 1.0 (3.0 / 3.0)
    assert!((output[1] - 1.0).abs() < 1e-6);
    assert!((output[3] - 1.0).abs() < 1e-6);
}

#[test]
fn test_convolve_row_matches_scalar() {
    let input: Vec<f32> = (0..100).map(|i| i as f32 * 0.1).collect();
    let kernel = vec![0.1, 0.2, 0.4, 0.2, 0.1]; // Gaussian-like
    let radius = 2;

    let mut output_simd = vec![0.0f32; 100];
    let mut output_scalar = vec![0.0f32; 100];

    convolve_row(&input, &mut output_simd, &kernel, radius);
    convolve_row_scalar(&input, &mut output_scalar, &kernel, radius);

    for i in 0..100 {
        assert!(
            (output_simd[i] - output_scalar[i]).abs() < 1e-5,
            "SIMD and scalar should match at index {}: {} vs {}",
            i,
            output_simd[i],
            output_scalar[i]
        );
    }
}

// ========== Comprehensive SIMD vs Scalar tests ==========

#[test]
fn test_convolve_row_small_input_less_than_simd_width() {
    // Test inputs smaller than SIMD register width
    // Start at width=3 because mirror boundary requires at least 3 pixels for radius=1
    for width in 3..16 {
        let input: Vec<f32> = (0..width).map(|i| i as f32 + 1.0).collect();
        let kernel = vec![0.25, 0.5, 0.25];
        let radius = 1;

        let mut output_simd = vec![0.0f32; width];
        let mut output_scalar = vec![0.0f32; width];

        convolve_row(&input, &mut output_simd, &kernel, radius);
        convolve_row_scalar(&input, &mut output_scalar, &kernel, radius);

        for i in 0..width {
            assert!(
                (output_simd[i] - output_scalar[i]).abs() < 1e-5,
                "Width {}, index {}: {} vs {}",
                width,
                i,
                output_simd[i],
                output_scalar[i]
            );
        }
    }
}

#[test]
fn test_convolve_row_edge_boundary_handling() {
    // Test that boundary mirror handling works correctly
    let input = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
    let kernel = vec![0.2, 0.3, 0.3, 0.2]; // radius 1, asymmetric kernel
    let radius = 1;

    let mut output_simd = vec![0.0f32; 8];
    let mut output_scalar = vec![0.0f32; 8];

    convolve_row(&input, &mut output_simd, &kernel, radius);
    convolve_row_scalar(&input, &mut output_scalar, &kernel, radius);

    // Check all pixels including edges
    for i in 0..8 {
        assert!(
            (output_simd[i] - output_scalar[i]).abs() < 1e-5,
            "Edge handling mismatch at {}: {} vs {}",
            i,
            output_simd[i],
            output_scalar[i]
        );
    }
}

#[test]
fn test_convolve_row_large_kernel() {
    // Test with a larger kernel (radius 5, kernel size 11)
    let input: Vec<f32> = (0..100).map(|i| (i as f32).sin() * 10.0 + 50.0).collect();
    let kernel: Vec<f32> = (0..11)
        .map(|i| 1.0 / 11.0 * (1.0 + 0.1 * i as f32))
        .collect();
    let kernel_sum: f32 = kernel.iter().sum();
    let kernel: Vec<f32> = kernel.iter().map(|&k| k / kernel_sum).collect(); // Normalize
    let radius = 5;

    let mut output_simd = vec![0.0f32; 100];
    let mut output_scalar = vec![0.0f32; 100];

    convolve_row(&input, &mut output_simd, &kernel, radius);
    convolve_row_scalar(&input, &mut output_scalar, &kernel, radius);

    for i in 0..100 {
        assert!(
            (output_simd[i] - output_scalar[i]).abs() < 1e-4,
            "Large kernel mismatch at {}: {} vs {}",
            i,
            output_simd[i],
            output_scalar[i]
        );
    }
}

#[test]
fn test_convolve_row_various_kernel_radii() {
    let input: Vec<f32> = (0..64).map(|i| i as f32 * 0.5).collect();

    for radius in 1..=8 {
        let kernel_size = radius * 2 + 1;
        let kernel: Vec<f32> = vec![1.0 / kernel_size as f32; kernel_size];

        let mut output_simd = vec![0.0f32; 64];
        let mut output_scalar = vec![0.0f32; 64];

        convolve_row(&input, &mut output_simd, &kernel, radius);
        convolve_row_scalar(&input, &mut output_scalar, &kernel, radius);

        for i in 0..64 {
            assert!(
                (output_simd[i] - output_scalar[i]).abs() < 1e-4,
                "Radius {}, index {}: {} vs {}",
                radius,
                i,
                output_simd[i],
                output_scalar[i]
            );
        }
    }
}

#[test]
fn test_convolve_row_uniform_input() {
    let input = vec![42.0; 128];
    let kernel = vec![0.1, 0.2, 0.4, 0.2, 0.1]; // Gaussian-like, sums to 1.0
    let radius = 2;

    let mut output_simd = vec![0.0f32; 128];
    let mut output_scalar = vec![0.0f32; 128];

    convolve_row(&input, &mut output_simd, &kernel, radius);
    convolve_row_scalar(&input, &mut output_scalar, &kernel, radius);

    for i in 0..128 {
        assert!(
            (output_simd[i] - output_scalar[i]).abs() < 1e-5,
            "Uniform input mismatch at {}: {} vs {}",
            i,
            output_simd[i],
            output_scalar[i]
        );
        // With uniform input and normalized kernel, output should equal input
        assert!(
            (output_simd[i] - 42.0).abs() < 1e-5,
            "Uniform input should remain unchanged: {}",
            output_simd[i]
        );
    }
}

#[test]
fn test_convolve_row_impulse_response() {
    // Single impulse in the middle
    let mut input = vec![0.0f32; 64];
    input[32] = 1.0;

    let kernel = vec![0.1, 0.2, 0.4, 0.2, 0.1];
    let radius = 2;

    let mut output_simd = vec![0.0f32; 64];
    let mut output_scalar = vec![0.0f32; 64];

    convolve_row(&input, &mut output_simd, &kernel, radius);
    convolve_row_scalar(&input, &mut output_scalar, &kernel, radius);

    for i in 0..64 {
        assert!(
            (output_simd[i] - output_scalar[i]).abs() < 1e-5,
            "Impulse response mismatch at {}: {} vs {}",
            i,
            output_simd[i],
            output_scalar[i]
        );
    }

    // Verify impulse response matches kernel
    for (k, &kval) in kernel.iter().enumerate() {
        let idx = 32 - radius + k;
        assert!(
            (output_simd[idx] - kval).abs() < 1e-5,
            "Impulse at {} should match kernel[{}]: {} vs {}",
            idx,
            k,
            output_simd[idx],
            kval
        );
    }
}

#[test]
fn test_convolve_row_negative_values() {
    let input: Vec<f32> = (-50..50).map(|i| i as f32 * 0.1).collect();
    let kernel = vec![0.15, 0.25, 0.2, 0.25, 0.15];
    let radius = 2;

    let mut output_simd = vec![0.0f32; 100];
    let mut output_scalar = vec![0.0f32; 100];

    convolve_row(&input, &mut output_simd, &kernel, radius);
    convolve_row_scalar(&input, &mut output_scalar, &kernel, radius);

    for i in 0..100 {
        assert!(
            (output_simd[i] - output_scalar[i]).abs() < 1e-5,
            "Negative values mismatch at {}: {} vs {}",
            i,
            output_simd[i],
            output_scalar[i]
        );
    }
}

#[test]
fn test_convolve_row_large_array() {
    // Test with larger array to exercise SIMD loop iterations
    let input: Vec<f32> = (0..1024).map(|i| (i as f32 * 0.1).sin() * 100.0).collect();
    let kernel = vec![0.05, 0.1, 0.2, 0.3, 0.2, 0.1, 0.05];
    let radius = 3;

    let mut output_simd = vec![0.0f32; 1024];
    let mut output_scalar = vec![0.0f32; 1024];

    convolve_row(&input, &mut output_simd, &kernel, radius);
    convolve_row_scalar(&input, &mut output_scalar, &kernel, radius);

    for i in 0..1024 {
        assert!(
            (output_simd[i] - output_scalar[i]).abs() < 1e-4,
            "Large array mismatch at {}: {} vs {}",
            i,
            output_simd[i],
            output_scalar[i]
        );
    }
}

#[test]
fn test_convolve_row_random_pattern() {
    // Pseudo-random pattern using sin/cos combination
    let input: Vec<f32> = (0..256)
        .map(|i| (i as f32 * 0.7).sin() * (i as f32 * 0.3).cos() * 50.0 + 100.0)
        .collect();
    let kernel = vec![0.1, 0.15, 0.25, 0.25, 0.15, 0.1];
    let radius = 2;

    let mut output_simd = vec![0.0f32; 256];
    let mut output_scalar = vec![0.0f32; 256];

    convolve_row(&input, &mut output_simd, &kernel, radius);
    convolve_row_scalar(&input, &mut output_scalar, &kernel, radius);

    for i in 0..256 {
        assert!(
            (output_simd[i] - output_scalar[i]).abs() < 1e-4,
            "Random pattern mismatch at {}: {} vs {}",
            i,
            output_simd[i],
            output_scalar[i]
        );
    }
}

#[test]
fn test_convolve_row_edge_only() {
    // Test very small arrays where only edges exist
    let input = vec![1.0, 2.0, 3.0];
    let kernel = vec![0.25, 0.5, 0.25];
    let radius = 1;

    let mut output_simd = vec![0.0f32; 3];
    let mut output_scalar = vec![0.0f32; 3];

    convolve_row(&input, &mut output_simd, &kernel, radius);
    convolve_row_scalar(&input, &mut output_scalar, &kernel, radius);

    for i in 0..3 {
        assert!(
            (output_simd[i] - output_scalar[i]).abs() < 1e-5,
            "Tiny array mismatch at {}: {} vs {}",
            i,
            output_simd[i],
            output_scalar[i]
        );
    }
}
