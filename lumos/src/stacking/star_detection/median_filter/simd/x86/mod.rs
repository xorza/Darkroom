//! SSE4.1 and AVX2 SIMD implementations for 3x3 median filter.
//!
//! Uses vectorized min/max operations to implement the sorting network.
//! Each SIMD register processes multiple independent median computations.

#![allow(clippy::needless_range_loop)]
#![allow(unused_assignments)] // Sorting network leaves some values unused

#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;

use crate::stacking::star_detection::median_filter::simd;

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
pub(super) unsafe fn median_filter_row_avx2(
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
            output_row[x] = simd::median9_scalar(
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
pub(super) unsafe fn median_filter_row_sse41(
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
            output_row[x] = simd::median9_scalar(
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
    median9_simd_sort!(_mm256_min_ps, _mm256_max_ps; v0, v1, v2, v3, v4, v5, v6, v7, v8);
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
    median9_simd_sort!(_mm_min_ps, _mm_max_ps; v0, v1, v2, v3, v4, v5, v6, v7, v8);
    v4
}

#[cfg(test)]
mod tests;
