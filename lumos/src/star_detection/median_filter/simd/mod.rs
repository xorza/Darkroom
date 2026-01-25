//! SIMD-accelerated 3x3 median filter.
//!
//! This module provides runtime dispatch to the best available SIMD implementation:
//! - AVX2/SSE4.1 on x86_64
//! - NEON on aarch64
//! - Scalar fallback on other platforms
//!
//! SIMD acceleration for median filtering is limited because:
//! 1. Median computation requires sorting/comparison networks
//! 2. Each pixel needs its own 9-element neighborhood
//! 3. Memory access patterns are not purely sequential
//!
//! The main benefit comes from processing multiple rows in parallel and
//! using SIMD for the min/max operations in the sorting network.

#[cfg(target_arch = "x86_64")]
use crate::common::cpu_features;

#[cfg(target_arch = "x86_64")]
pub mod sse;

#[cfg(target_arch = "aarch64")]
pub mod neon;

/// Process a row of interior pixels using SIMD-accelerated median9.
///
/// This function dispatches to the best available SIMD implementation at runtime.
/// Falls back to scalar for small widths or unsupported platforms.
///
/// # Arguments
/// * `row_above` - Pointer to start of row above (y-1)
/// * `row_curr` - Pointer to start of current row (y)
/// * `row_below` - Pointer to start of row below (y+1)
/// * `output_row` - Output buffer for this row
/// * `width` - Image width
#[inline]
pub fn median_filter_row_simd(
    row_above: &[f32],
    row_curr: &[f32],
    row_below: &[f32],
    output_row: &mut [f32],
    width: usize,
) {
    #[cfg(target_arch = "x86_64")]
    {
        if width >= 12 && cpu_features::has_avx2() {
            unsafe {
                sse::median_filter_row_avx2(row_above, row_curr, row_below, output_row, width);
            }
            return;
        }
        if width >= 8 && cpu_features::has_sse4_1() {
            unsafe {
                sse::median_filter_row_sse41(row_above, row_curr, row_below, output_row, width);
            }
            return;
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        if width >= 8 {
            unsafe {
                neon::median_filter_row_neon(row_above, row_curr, row_below, output_row, width);
            }
            return;
        }
    }

    // Scalar fallback
    #[allow(unreachable_code)]
    median_filter_row_scalar(row_above, row_curr, row_below, output_row, width);
}

/// Scalar implementation of median filter row processing.
#[inline]
pub fn median_filter_row_scalar(
    row_above: &[f32],
    row_curr: &[f32],
    row_below: &[f32],
    output_row: &mut [f32],
    width: usize,
) {
    for x in 1..width - 1 {
        output_row[x] = median9_scalar(
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

/// Scalar median of 9 elements using sorting network.
#[inline]
#[allow(clippy::too_many_arguments)]
pub fn median9_scalar(
    mut v0: f32,
    mut v1: f32,
    mut v2: f32,
    mut v3: f32,
    mut v4: f32,
    mut v5: f32,
    mut v6: f32,
    mut v7: f32,
    mut v8: f32,
) -> f32 {
    // Optimal 25-comparator sorting network for 9 elements
    macro_rules! swap {
        ($a:expr, $b:expr) => {
            if $a > $b {
                std::mem::swap(&mut $a, &mut $b);
            }
        };
    }

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

    #[test]
    fn test_median_filter_row_scalar() {
        let width = 16;
        let row_above: Vec<f32> = (0..width).map(|i| (i % 10) as f32 * 0.1).collect();
        let row_curr: Vec<f32> = (0..width).map(|i| ((i + 3) % 10) as f32 * 0.1).collect();
        let row_below: Vec<f32> = (0..width).map(|i| ((i + 7) % 10) as f32 * 0.1).collect();
        let mut output = vec![0.0f32; width];

        median_filter_row_scalar(&row_above, &row_curr, &row_below, &mut output, width);

        // Verify a specific pixel manually
        let x = 5;
        let mut values = [
            row_above[x - 1],
            row_above[x],
            row_above[x + 1],
            row_curr[x - 1],
            row_curr[x],
            row_curr[x + 1],
            row_below[x - 1],
            row_below[x],
            row_below[x + 1],
        ];
        values.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let expected = values[4];

        assert!(
            (output[x] - expected).abs() < 1e-6,
            "Scalar median mismatch at x={}: got {}, expected {}",
            x,
            output[x],
            expected
        );
    }

    #[test]
    fn test_median_filter_row_simd_matches_scalar() {
        let width = 64;
        let row_above: Vec<f32> = (0..width).map(|i| ((i * 3) % 100) as f32 * 0.01).collect();
        let row_curr: Vec<f32> = (0..width).map(|i| ((i * 7) % 100) as f32 * 0.01).collect();
        let row_below: Vec<f32> = (0..width).map(|i| ((i * 11) % 100) as f32 * 0.01).collect();

        let mut output_scalar = vec![0.0f32; width];
        let mut output_simd = vec![0.0f32; width];

        median_filter_row_scalar(&row_above, &row_curr, &row_below, &mut output_scalar, width);
        median_filter_row_simd(&row_above, &row_curr, &row_below, &mut output_simd, width);

        for x in 1..width - 1 {
            assert!(
                (output_simd[x] - output_scalar[x]).abs() < 1e-5,
                "SIMD vs scalar mismatch at x={}: {} vs {}",
                x,
                output_simd[x],
                output_scalar[x]
            );
        }
    }

    #[test]
    fn test_median9_scalar_known_values() {
        // Sorted: 0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9
        // Median should be 0.5 (index 4)
        let result = median9_scalar(0.5, 0.1, 0.9, 0.2, 0.8, 0.3, 0.7, 0.4, 0.6);
        assert!((result - 0.5).abs() < 1e-6, "Expected 0.5, got {}", result);
    }

    #[test]
    fn test_median9_scalar_all_same() {
        let result = median9_scalar(0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5);
        assert!((result - 0.5).abs() < 1e-6, "Expected 0.5, got {}", result);
    }
}
