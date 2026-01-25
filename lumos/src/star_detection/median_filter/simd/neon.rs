//! NEON SIMD implementation for 3x3 median filter on aarch64.

#![allow(clippy::needless_range_loop)]

#[cfg(target_arch = "aarch64")]
use std::arch::aarch64::*;

/// Process a row of interior pixels using NEON.
///
/// Processes 4 pixels in parallel.
///
/// # Safety
/// - Caller must ensure this is running on aarch64.
/// - `width` must be >= 8 (4 SIMD + edges).
#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
pub unsafe fn median_filter_row_neon(
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
            let a0 = vld1q_f32(ptr_above.add(x - 1));
            let a1 = vld1q_f32(ptr_above.add(x));
            let a2 = vld1q_f32(ptr_above.add(x + 1));

            let c0 = vld1q_f32(ptr_curr.add(x - 1));
            let c1 = vld1q_f32(ptr_curr.add(x));
            let c2 = vld1q_f32(ptr_curr.add(x + 1));

            let b0 = vld1q_f32(ptr_below.add(x - 1));
            let b1 = vld1q_f32(ptr_below.add(x));
            let b2 = vld1q_f32(ptr_below.add(x + 1));

            // Apply sorting network to find median
            let result = median9_neon(a0, a1, a2, c0, c1, c2, b0, b1, b2);

            vst1q_f32(out_ptr.add(x), result);
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

/// Vectorized median of 9 elements using NEON.
#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
#[inline]
unsafe fn median9_neon(
    mut v0: float32x4_t,
    mut v1: float32x4_t,
    mut v2: float32x4_t,
    mut v3: float32x4_t,
    mut v4: float32x4_t,
    mut v5: float32x4_t,
    mut v6: float32x4_t,
    mut v7: float32x4_t,
    mut v8: float32x4_t,
) -> float32x4_t {
    // Sorting network using min/max
    macro_rules! swap {
        ($a:ident, $b:ident) => {
            let t = $a;
            $a = vminq_f32($a, $b);
            $b = vmaxq_f32(t, $b);
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
    #[allow(unused_imports)]
    use super::*;

    #[test]
    #[cfg(target_arch = "aarch64")]
    fn test_neon_median_filter_row() {
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
            median_filter_row_neon(&row_above, &row_curr, &row_below, &mut output_simd, width);
        }

        for x in 1..width - 1 {
            assert!(
                (output_simd[x] - output_scalar[x]).abs() < 1e-5,
                "NEON mismatch at x={}: {} vs {}",
                x,
                output_simd[x],
                output_scalar[x]
            );
        }
    }
}
