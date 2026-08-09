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

use crate::simd::dispatch;

/// 25-comparator sort network for 9 elements, parameterized by the lane type's `min`/`max` so the
/// AVX2/SSE4.1/NEON kernels share one definition. After it runs, `$v4` holds the per-lane median.
/// (`median9_scalar` deliberately stays a separate, independently-written reference so the
/// SIMD-vs-scalar cross-check tests validate the network rather than just re-checking this macro.)
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
macro_rules! median9_simd_sort {
    ($min:path, $max:path;
     $v0:ident, $v1:ident, $v2:ident, $v3:ident, $v4:ident, $v5:ident, $v6:ident, $v7:ident, $v8:ident) => {{
        // Each step: (a, b) => a = min(a, b), b = max(a, b), via a temp so `a` isn't read twice.
        let t = $v0;
        $v0 = $min($v0, $v1);
        $v1 = $max(t, $v1);
        let t = $v3;
        $v3 = $min($v3, $v4);
        $v4 = $max(t, $v4);
        let t = $v6;
        $v6 = $min($v6, $v7);
        $v7 = $max(t, $v7);
        let t = $v1;
        $v1 = $min($v1, $v2);
        $v2 = $max(t, $v2);
        let t = $v4;
        $v4 = $min($v4, $v5);
        $v5 = $max(t, $v5);
        let t = $v7;
        $v7 = $min($v7, $v8);
        $v8 = $max(t, $v8);
        let t = $v0;
        $v0 = $min($v0, $v1);
        $v1 = $max(t, $v1);
        let t = $v3;
        $v3 = $min($v3, $v4);
        $v4 = $max(t, $v4);
        let t = $v6;
        $v6 = $min($v6, $v7);
        $v7 = $max(t, $v7);
        let t = $v0;
        $v0 = $min($v0, $v3);
        $v3 = $max(t, $v3);
        let t = $v3;
        $v3 = $min($v3, $v6);
        $v6 = $max(t, $v6);
        let t = $v0;
        $v0 = $min($v0, $v3);
        $v3 = $max(t, $v3);
        let t = $v1;
        $v1 = $min($v1, $v4);
        $v4 = $max(t, $v4);
        let t = $v4;
        $v4 = $min($v4, $v7);
        $v7 = $max(t, $v7);
        let t = $v1;
        $v1 = $min($v1, $v4);
        $v4 = $max(t, $v4);
        let t = $v2;
        $v2 = $min($v2, $v5);
        $v5 = $max(t, $v5);
        let t = $v5;
        $v5 = $min($v5, $v8);
        $v8 = $max(t, $v8);
        let t = $v2;
        $v2 = $min($v2, $v5);
        $v5 = $max(t, $v5);
        let t = $v1;
        $v1 = $min($v1, $v3);
        $v3 = $max(t, $v3);
        let t = $v5;
        $v5 = $min($v5, $v7);
        $v7 = $max(t, $v7);
        let t = $v2;
        $v2 = $min($v2, $v6);
        $v6 = $max(t, $v6);
        let t = $v4;
        $v4 = $min($v4, $v6);
        $v6 = $max(t, $v6);
        let t = $v2;
        $v2 = $min($v2, $v4);
        $v4 = $max(t, $v4);
        let t = $v2;
        $v2 = $min($v2, $v3);
        $v3 = $max(t, $v3);
        let t = $v4;
        $v4 = $min($v4, $v5);
        $v5 = $max(t, $v5);
    }};
}

#[cfg(target_arch = "x86_64")]
mod x86;

#[cfg(target_arch = "aarch64")]
mod neon;

/// Row widths at or above which each kernel is entered. Every backend vectorizes the interior
/// pixels as `(width - 2) / LANES` chunks, so a row only just wider than `2 + LANES` is nearly
/// all scalar remainder; these sit above that break-even rather than at it.
#[cfg(target_arch = "x86_64")]
const AVX2_MIN_ROW_WIDTH: usize = 12;
#[cfg(target_arch = "x86_64")]
const SSE41_MIN_ROW_WIDTH: usize = 8;
#[cfg(target_arch = "aarch64")]
const NEON_MIN_ROW_WIDTH: usize = 8;

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
pub(super) fn median_filter_row_simd(
    row_above: &[f32],
    row_curr: &[f32],
    row_below: &[f32],
    output_row: &mut [f32],
    width: usize,
) {
    dispatch! {
        x86: avx2 if width >= AVX2_MIN_ROW_WIDTH
            => x86::median_filter_row_avx2(row_above, row_curr, row_below, output_row, width),
        x86: sse4_1 if width >= SSE41_MIN_ROW_WIDTH
            => x86::median_filter_row_sse41(row_above, row_curr, row_below, output_row, width),
        aarch64 if width >= NEON_MIN_ROW_WIDTH
            => neon::median_filter_row_neon(row_above, row_curr, row_below, output_row, width),
        scalar => median_filter_row_scalar(row_above, row_curr, row_below, output_row, width),
    }
}

/// Scalar implementation of median filter row processing.
#[inline]
fn median_filter_row_scalar(
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
fn median9_scalar(
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
mod tests;
