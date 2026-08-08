use crate::stacking::star_detection::convolution::simd;

use crate::stacking::star_detection::convolution::simd::x86::*;
use imaginarium::cpu_features;

#[test]
fn test_avx2_matches_scalar() {
    if !cpu_features::has_avx2_fma() {
        eprintln!("Skipping AVX2 test: CPU does not support AVX2+FMA");
        return;
    }

    let input: Vec<f32> = (0..256).map(|i| (i as f32).sin()).collect();
    let kernel = vec![0.05, 0.1, 0.2, 0.3, 0.2, 0.1, 0.05];
    let radius = 3;

    let mut output_avx2 = vec![0.0f32; 256];
    let mut output_scalar = vec![0.0f32; 256];

    unsafe {
        convolve_row_avx2(&input, &mut output_avx2, &kernel, radius);
    }

    for x in 0..256 {
        output_scalar[x] = convolve_pixel_scalar(&input, &kernel, radius, x, 256);
    }

    for i in 0..256 {
        assert!(
            (output_avx2[i] - output_scalar[i]).abs() < 1e-5,
            "AVX2 mismatch at {}: {} vs {}",
            i,
            output_avx2[i],
            output_scalar[i]
        );
    }
}

#[test]
fn test_sse41_matches_scalar() {
    if !cpu_features::has_sse4_1() {
        eprintln!("Skipping SSE4.1 test: CPU does not support SSE4.1");
        return;
    }

    let input: Vec<f32> = (0..256).map(|i| (i as f32).sin()).collect();
    let kernel = vec![0.05, 0.1, 0.2, 0.3, 0.2, 0.1, 0.05];
    let radius = 3;

    let mut output_sse = vec![0.0f32; 256];
    let mut output_scalar = vec![0.0f32; 256];

    unsafe {
        convolve_row_sse41(&input, &mut output_sse, &kernel, radius);
    }

    for x in 0..256 {
        output_scalar[x] = convolve_pixel_scalar(&input, &kernel, radius, x, 256);
    }

    for i in 0..256 {
        assert!(
            (output_sse[i] - output_scalar[i]).abs() < 1e-5,
            "SSE4.1 mismatch at {}: {} vs {}",
            i,
            output_sse[i],
            output_scalar[i]
        );
    }
}

#[test]
fn test_avx2_cols_matches_scalar() {
    if !cpu_features::has_avx2_fma() {
        eprintln!("Skipping AVX2 cols test: CPU does not support AVX2+FMA");
        return;
    }

    let width = 64;
    let height = 64;
    let input: Vec<f32> = (0..width * height)
        .map(|i| (i as f32 * 0.1).sin())
        .collect();
    let kernel = vec![0.05, 0.1, 0.2, 0.3, 0.2, 0.1, 0.05];
    let radius = 3;

    let mut output_avx2 = vec![0.0f32; width * height];
    let mut output_scalar = vec![0.0f32; width * height];

    unsafe {
        for y in 0..height {
            convolve_cols_row_avx2(
                &input,
                &mut output_avx2[y * width..(y + 1) * width],
                Size2us::new(width, height),
                y,
                &kernel,
                radius,
            );
        }
    }

    // Scalar reference
    for x in 0..width {
        for y in 0..height {
            let mut sum = 0.0f32;
            for (k, &kval) in kernel.iter().enumerate() {
                let sy = y as isize + k as isize - radius as isize;
                let sy = simd::mirror_index(sy, height);
                sum += input[sy * width + x] * kval;
            }
            output_scalar[y * width + x] = sum;
        }
    }

    for i in 0..width * height {
        assert!(
            (output_avx2[i] - output_scalar[i]).abs() < 1e-5,
            "AVX2 cols mismatch at {}: {} vs {}",
            i,
            output_avx2[i],
            output_scalar[i]
        );
    }
}

#[test]
fn test_sse41_cols_matches_scalar() {
    if !cpu_features::has_sse4_1() {
        eprintln!("Skipping SSE4.1 cols test: CPU does not support SSE4.1");
        return;
    }

    let width = 64;
    let height = 64;
    let input: Vec<f32> = (0..width * height)
        .map(|i| (i as f32 * 0.1).sin())
        .collect();
    let kernel = vec![0.05, 0.1, 0.2, 0.3, 0.2, 0.1, 0.05];
    let radius = 3;

    let mut output_sse = vec![0.0f32; width * height];
    let mut output_scalar = vec![0.0f32; width * height];

    unsafe {
        for y in 0..height {
            convolve_cols_row_sse41(
                &input,
                &mut output_sse[y * width..(y + 1) * width],
                Size2us::new(width, height),
                y,
                &kernel,
                radius,
            );
        }
    }

    // Scalar reference
    for x in 0..width {
        for y in 0..height {
            let mut sum = 0.0f32;
            for (k, &kval) in kernel.iter().enumerate() {
                let sy = y as isize + k as isize - radius as isize;
                let sy = simd::mirror_index(sy, height);
                sum += input[sy * width + x] * kval;
            }
            output_scalar[y * width + x] = sum;
        }
    }

    for i in 0..width * height {
        assert!(
            (output_sse[i] - output_scalar[i]).abs() < 1e-5,
            "SSE4.1 cols mismatch at {}: {} vs {}",
            i,
            output_sse[i],
            output_scalar[i]
        );
    }
}
