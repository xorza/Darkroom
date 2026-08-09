use crate::stacking::star_detection::median_filter::simd::x86::*;
use imaginarium::cpu_features;

fn median9_reference(values: &mut [f32; 9]) -> f32 {
    values.sort_by(|a, b| a.partial_cmp(b).unwrap());
    values[4]
}

#[test]
fn test_avx2_median9() {
    if !cpu_features::has_avx2() {
        eprintln!("Skipping AVX2 test - not available");
        return;
    }

    // Test with 8 independent median computations
    let test_cases: [[f32; 9]; 8] = [
        [0.5, 0.1, 0.9, 0.2, 0.8, 0.3, 0.7, 0.4, 0.6], // median = 0.5
        [0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9], // median = 0.5
        [0.9, 0.8, 0.7, 0.6, 0.5, 0.4, 0.3, 0.2, 0.1], // median = 0.5
        [1.0, 1.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.5], // median = 0.5
        [0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0, 0.5], // median = 0.5
        [0.3, 0.3, 0.3, 0.3, 0.3, 0.3, 0.3, 0.3, 0.3], // median = 0.3
        [0.1, 0.1, 0.1, 0.2, 0.2, 0.2, 0.3, 0.3, 0.3], // median = 0.2
        [0.0, 0.25, 0.5, 0.75, 1.0, 0.1, 0.2, 0.3, 0.4], // median = 0.3
    ];

    // Build input arrays (transposed for SIMD)
    let mut v: [Vec<f32>; 9] = Default::default();
    for i in 0..9 {
        v[i] = test_cases.iter().map(|tc| tc[i]).collect();
    }

    unsafe {
        let inputs: [__m256; 9] = [
            _mm256_loadu_ps(v[0].as_ptr()),
            _mm256_loadu_ps(v[1].as_ptr()),
            _mm256_loadu_ps(v[2].as_ptr()),
            _mm256_loadu_ps(v[3].as_ptr()),
            _mm256_loadu_ps(v[4].as_ptr()),
            _mm256_loadu_ps(v[5].as_ptr()),
            _mm256_loadu_ps(v[6].as_ptr()),
            _mm256_loadu_ps(v[7].as_ptr()),
            _mm256_loadu_ps(v[8].as_ptr()),
        ];

        let result = median9_avx2(
            inputs[0], inputs[1], inputs[2], inputs[3], inputs[4], inputs[5], inputs[6], inputs[7],
            inputs[8],
        );

        let mut output = [0.0f32; 8];
        _mm256_storeu_ps(output.as_mut_ptr(), result);

        for (i, tc) in test_cases.iter().enumerate() {
            let mut sorted = *tc;
            let expected = median9_reference(&mut sorted);
            assert!(
                (output[i] - expected).abs() < 1e-6,
                "Test case {}: expected {}, got {}",
                i,
                expected,
                output[i]
            );
        }
    }
}

#[test]
fn test_sse41_median9() {
    if !cpu_features::has_sse4_1() {
        eprintln!("Skipping SSE4.1 test - not available");
        return;
    }

    let test_cases: [[f32; 9]; 4] = [
        [0.5, 0.1, 0.9, 0.2, 0.8, 0.3, 0.7, 0.4, 0.6],
        [0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9],
        [0.9, 0.8, 0.7, 0.6, 0.5, 0.4, 0.3, 0.2, 0.1],
        [0.3, 0.3, 0.3, 0.3, 0.3, 0.3, 0.3, 0.3, 0.3],
    ];

    let mut v: [Vec<f32>; 9] = Default::default();
    for i in 0..9 {
        v[i] = test_cases.iter().map(|tc| tc[i]).collect();
    }

    unsafe {
        let inputs: [__m128; 9] = [
            _mm_loadu_ps(v[0].as_ptr()),
            _mm_loadu_ps(v[1].as_ptr()),
            _mm_loadu_ps(v[2].as_ptr()),
            _mm_loadu_ps(v[3].as_ptr()),
            _mm_loadu_ps(v[4].as_ptr()),
            _mm_loadu_ps(v[5].as_ptr()),
            _mm_loadu_ps(v[6].as_ptr()),
            _mm_loadu_ps(v[7].as_ptr()),
            _mm_loadu_ps(v[8].as_ptr()),
        ];

        let result = median9_sse41(
            inputs[0], inputs[1], inputs[2], inputs[3], inputs[4], inputs[5], inputs[6], inputs[7],
            inputs[8],
        );

        let mut output = [0.0f32; 4];
        _mm_storeu_ps(output.as_mut_ptr(), result);

        for (i, tc) in test_cases.iter().enumerate() {
            let mut sorted = *tc;
            let expected = median9_reference(&mut sorted);
            assert!(
                (output[i] - expected).abs() < 1e-6,
                "Test case {}: expected {}, got {}",
                i,
                expected,
                output[i]
            );
        }
    }
}

#[test]
fn test_median_filter_row_avx2() {
    if !cpu_features::has_avx2() {
        eprintln!("Skipping AVX2 row test - not available");
        return;
    }

    let width = 32;
    let row_above: Vec<f32> = (0..width).map(|i| ((i * 3) % 100) as f32 * 0.01).collect();
    let row_curr: Vec<f32> = (0..width).map(|i| ((i * 7) % 100) as f32 * 0.01).collect();
    let row_below: Vec<f32> = (0..width).map(|i| ((i * 11) % 100) as f32 * 0.01).collect();

    let mut output_scalar = vec![0.0f32; width];
    let mut output_simd = vec![0.0f32; width];

    simd::median_filter_row_scalar(&row_above, &row_curr, &row_below, &mut output_scalar, width);

    unsafe {
        median_filter_row_avx2(&row_above, &row_curr, &row_below, &mut output_simd, width);
    }

    for x in 1..width - 1 {
        assert!(
            (output_simd[x] - output_scalar[x]).abs() < 1e-5,
            "AVX2 mismatch at x={}: {} vs {}",
            x,
            output_simd[x],
            output_scalar[x]
        );
    }
}

#[test]
fn test_median_filter_row_sse41() {
    if !cpu_features::has_sse4_1() {
        eprintln!("Skipping SSE4.1 row test - not available");
        return;
    }

    let width = 20;
    let row_above: Vec<f32> = (0..width).map(|i| ((i * 3) % 100) as f32 * 0.01).collect();
    let row_curr: Vec<f32> = (0..width).map(|i| ((i * 7) % 100) as f32 * 0.01).collect();
    let row_below: Vec<f32> = (0..width).map(|i| ((i * 11) % 100) as f32 * 0.01).collect();

    let mut output_scalar = vec![0.0f32; width];
    let mut output_simd = vec![0.0f32; width];

    simd::median_filter_row_scalar(&row_above, &row_curr, &row_below, &mut output_scalar, width);

    unsafe {
        median_filter_row_sse41(&row_above, &row_curr, &row_below, &mut output_simd, width);
    }

    for x in 1..width - 1 {
        assert!(
            (output_simd[x] - output_scalar[x]).abs() < 1e-5,
            "SSE4.1 mismatch at x={}: {} vs {}",
            x,
            output_simd[x],
            output_scalar[x]
        );
    }
}
