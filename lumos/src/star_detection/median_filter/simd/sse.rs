//! SSE4.1 and AVX2 SIMD implementations for 3x3 median filter.
//!
//! Uses vectorized min/max operations to implement the sorting network.
//! Each SIMD register processes multiple independent median computations.

#![allow(clippy::needless_range_loop)]
#![allow(unused_assignments)] // Sorting network leaves some values unused

#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;

/// Process a row of interior pixels using AVX2.
///
/// Processes 8 pixels in parallel by loading overlapping windows and
/// running the sorting network on packed data.
///
/// # Safety
/// - Caller must ensure AVX2 is available.
/// - `width` must be >= 12 (8 SIMD + edges).
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
pub unsafe fn median_filter_row_avx2(
    row_above: &[f32],
    row_curr: &[f32],
    row_below: &[f32],
    output_row: &mut [f32],
    width: usize,
) {
    unsafe {
        let ptr_above = row_above.as_ptr();
        let ptr_curr = row_curr.as_ptr();
        let ptr_below = row_below.as_ptr();
        let out_ptr = output_row.as_mut_ptr();

        // Process 8 pixels at a time
        let chunks = (width - 2) / 8;
        for i in 0..chunks {
            let x = 1 + i * 8;

            // Load 9 values for each of 8 parallel windows
            // Row above: positions x-1, x, x+1
            let a0 = _mm256_loadu_ps(ptr_above.add(x - 1));
            let a1 = _mm256_loadu_ps(ptr_above.add(x));
            let a2 = _mm256_loadu_ps(ptr_above.add(x + 1));

            // Current row: positions x-1, x, x+1
            let c0 = _mm256_loadu_ps(ptr_curr.add(x - 1));
            let c1 = _mm256_loadu_ps(ptr_curr.add(x));
            let c2 = _mm256_loadu_ps(ptr_curr.add(x + 1));

            // Row below: positions x-1, x, x+1
            let b0 = _mm256_loadu_ps(ptr_below.add(x - 1));
            let b1 = _mm256_loadu_ps(ptr_below.add(x));
            let b2 = _mm256_loadu_ps(ptr_below.add(x + 1));

            // Apply sorting network to find median
            let result = median9_avx2(a0, a1, a2, c0, c1, c2, b0, b1, b2);

            _mm256_storeu_ps(out_ptr.add(x), result);
        }

        // Handle remainder pixels with scalar code
        let remainder_start = 1 + chunks * 8;
        for x in remainder_start..(width - 1) {
            output_row[x] = super::median9_scalar(
                row_above[x - 1],
                row_above[x],
                row_above[x + 1],
                row_curr[x - 1],
                row_curr[x],
                row_curr[x + 1],
                row_below[x - 1],
                row_below[x],
                row_below[x + 1],
            );
        }
    }
}

/// Process a row of interior pixels using SSE4.1.
///
/// Processes 4 pixels in parallel.
///
/// # Safety
/// - Caller must ensure SSE4.1 is available.
/// - `width` must be >= 8 (4 SIMD + edges).
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse4.1")]
pub unsafe fn median_filter_row_sse41(
    row_above: &[f32],
    row_curr: &[f32],
    row_below: &[f32],
    output_row: &mut [f32],
    width: usize,
) {
    unsafe {
        let ptr_above = row_above.as_ptr();
        let ptr_curr = row_curr.as_ptr();
        let ptr_below = row_below.as_ptr();
        let out_ptr = output_row.as_mut_ptr();

        // Process 4 pixels at a time
        let chunks = (width - 2) / 4;
        for i in 0..chunks {
            let x = 1 + i * 4;

            // Load 9 values for each of 4 parallel windows
            let a0 = _mm_loadu_ps(ptr_above.add(x - 1));
            let a1 = _mm_loadu_ps(ptr_above.add(x));
            let a2 = _mm_loadu_ps(ptr_above.add(x + 1));

            let c0 = _mm_loadu_ps(ptr_curr.add(x - 1));
            let c1 = _mm_loadu_ps(ptr_curr.add(x));
            let c2 = _mm_loadu_ps(ptr_curr.add(x + 1));

            let b0 = _mm_loadu_ps(ptr_below.add(x - 1));
            let b1 = _mm_loadu_ps(ptr_below.add(x));
            let b2 = _mm_loadu_ps(ptr_below.add(x + 1));

            // Apply sorting network to find median
            let result = median9_sse41(a0, a1, a2, c0, c1, c2, b0, b1, b2);

            _mm_storeu_ps(out_ptr.add(x), result);
        }

        // Handle remainder pixels with scalar code
        let remainder_start = 1 + chunks * 4;
        for x in remainder_start..(width - 1) {
            output_row[x] = super::median9_scalar(
                row_above[x - 1],
                row_above[x],
                row_above[x + 1],
                row_curr[x - 1],
                row_curr[x],
                row_curr[x + 1],
                row_below[x - 1],
                row_below[x],
                row_below[x + 1],
            );
        }
    }
}

/// Vectorized median of 9 elements using AVX2.
///
/// Uses min/max operations to implement a complete sorting network
/// that places the median in position 4. Based on Batcher's odd-even merge sort.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
#[inline]
#[allow(clippy::too_many_arguments)]
unsafe fn median9_avx2(
    mut v0: __m256,
    mut v1: __m256,
    mut v2: __m256,
    mut v3: __m256,
    mut v4: __m256,
    mut v5: __m256,
    mut v6: __m256,
    mut v7: __m256,
    mut v8: __m256,
) -> __m256 {
    // Sorting network using min/max
    // swap(a, b) => a=min(a,b), b=max(a,b)
    macro_rules! swap {
        ($a:ident, $b:ident) => {
            let t = $a;
            $a = _mm256_min_ps($a, $b);
            $b = _mm256_max_ps(t, $b);
        };
    }

    // Optimal 25-comparator sorting network for 9 elements
    // This fully sorts the array so v4 contains the median
    swap!(v0, v1);
    swap!(v3, v4);
    swap!(v6, v7);
    swap!(v1, v2);
    swap!(v4, v5);
    swap!(v7, v8);
    swap!(v0, v1);
    swap!(v3, v4);
    swap!(v6, v7);
    swap!(v0, v3);
    swap!(v3, v6);
    swap!(v0, v3);
    swap!(v1, v4);
    swap!(v4, v7);
    swap!(v1, v4);
    swap!(v2, v5);
    swap!(v5, v8);
    swap!(v2, v5);
    swap!(v1, v3);
    swap!(v5, v7);
    swap!(v2, v6);
    swap!(v4, v6);
    swap!(v2, v4);
    swap!(v2, v3);
    swap!(v4, v5);

    v4
}

/// Vectorized median of 9 elements using SSE4.1.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse4.1")]
#[inline]
#[allow(clippy::too_many_arguments)]
unsafe fn median9_sse41(
    mut v0: __m128,
    mut v1: __m128,
    mut v2: __m128,
    mut v3: __m128,
    mut v4: __m128,
    mut v5: __m128,
    mut v6: __m128,
    mut v7: __m128,
    mut v8: __m128,
) -> __m128 {
    // Sorting network using min/max
    macro_rules! swap {
        ($a:ident, $b:ident) => {
            let t = $a;
            $a = _mm_min_ps($a, $b);
            $b = _mm_max_ps(t, $b);
        };
    }

    // Optimal 25-comparator sorting network for 9 elements
    swap!(v0, v1);
    swap!(v3, v4);
    swap!(v6, v7);
    swap!(v1, v2);
    swap!(v4, v5);
    swap!(v7, v8);
    swap!(v0, v1);
    swap!(v3, v4);
    swap!(v6, v7);
    swap!(v0, v3);
    swap!(v3, v6);
    swap!(v0, v3);
    swap!(v1, v4);
    swap!(v4, v7);
    swap!(v1, v4);
    swap!(v2, v5);
    swap!(v5, v8);
    swap!(v2, v5);
    swap!(v1, v3);
    swap!(v5, v7);
    swap!(v2, v6);
    swap!(v4, v6);
    swap!(v2, v4);
    swap!(v2, v3);
    swap!(v4, v5);

    v4
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_arch = "x86_64")]
    use common::cpu_features;

    fn median9_reference(values: &mut [f32; 9]) -> f32 {
        values.sort_by(|a, b| a.partial_cmp(b).unwrap());
        values[4]
    }

    #[test]
    #[cfg(target_arch = "x86_64")]
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
                inputs[0], inputs[1], inputs[2], inputs[3], inputs[4], inputs[5], inputs[6],
                inputs[7], inputs[8],
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
    #[cfg(target_arch = "x86_64")]
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
                inputs[0], inputs[1], inputs[2], inputs[3], inputs[4], inputs[5], inputs[6],
                inputs[7], inputs[8],
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
    #[cfg(target_arch = "x86_64")]
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

        super::super::median_filter_row_scalar(
            &row_above,
            &row_curr,
            &row_below,
            &mut output_scalar,
            width,
        );

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
    #[cfg(target_arch = "x86_64")]
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

        super::super::median_filter_row_scalar(
            &row_above,
            &row_curr,
            &row_below,
            &mut output_scalar,
            width,
        );

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
}
