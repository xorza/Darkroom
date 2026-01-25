//! ARM NEON implementations of background statistics functions.

#![allow(clippy::needless_range_loop)]

#[cfg(target_arch = "aarch64")]
use std::arch::aarch64::*;

/// Compute sum and sum of squares using NEON.
///
/// # Safety
/// Caller must ensure running on aarch64.
#[cfg(target_arch = "aarch64")]
pub unsafe fn sum_and_sum_sq_neon(values: &[f32]) -> (f32, f32) {
    unsafe {
        let len = values.len();
        let ptr = values.as_ptr();

        let mut sum_vec = vdupq_n_f32(0.0);
        let mut sum_sq_vec = vdupq_n_f32(0.0);

        // Process 4 floats at a time
        let chunks = len / 4;
        for i in 0..chunks {
            let v = vld1q_f32(ptr.add(i * 4));
            sum_vec = vaddq_f32(sum_vec, v);
            sum_sq_vec = vfmaq_f32(sum_sq_vec, v, v);
        }

        // Horizontal sum
        let sum = vaddvq_f32(sum_vec);
        let sum_sq = vaddvq_f32(sum_sq_vec);

        // Handle remainder
        let remainder_start = chunks * 4;
        let mut sum_rem = 0.0f32;
        let mut sum_sq_rem = 0.0f32;
        for i in remainder_start..len {
            let v = *ptr.add(i);
            sum_rem += v;
            sum_sq_rem += v * v;
        }

        (sum + sum_rem, sum_sq + sum_sq_rem)
    }
}

/// Compute sum of absolute deviations from median using NEON.
///
/// # Safety
/// Caller must ensure running on aarch64.
#[cfg(target_arch = "aarch64")]
pub unsafe fn sum_abs_deviations_neon(values: &[f32], median: f32) -> f32 {
    unsafe {
        let len = values.len();
        let ptr = values.as_ptr();

        let median_vec = vdupq_n_f32(median);
        let mut sum_vec = vdupq_n_f32(0.0);

        // Process 4 floats at a time
        let chunks = len / 4;
        for i in 0..chunks {
            let v = vld1q_f32(ptr.add(i * 4));
            let diff = vsubq_f32(v, median_vec);
            let abs_diff = vabsq_f32(diff);
            sum_vec = vaddq_f32(sum_vec, abs_diff);
        }

        let mut sum = vaddvq_f32(sum_vec);

        // Handle remainder
        let remainder_start = chunks * 4;
        for i in remainder_start..len {
            sum += (*ptr.add(i) - median).abs();
        }

        sum
    }
}

#[cfg(test)]
#[cfg(target_arch = "aarch64")]
mod tests {
    use super::*;

    #[test]
    fn test_neon_sum_and_sum_sq() {
        let values: Vec<f32> = (1..=100).map(|i| i as f32).collect();
        let (sum, sum_sq) = unsafe { sum_and_sum_sq_neon(&values) };

        let expected_sum: f32 = (1..=100).map(|i| i as f32).sum();
        let expected_sum_sq: f32 = (1..=100).map(|i| (i * i) as f32).sum();

        assert!(
            (sum - expected_sum).abs() < 1e-3,
            "Sum: {} vs {}",
            sum,
            expected_sum
        );
        assert!(
            (sum_sq - expected_sum_sq).abs() < 1.0,
            "Sum sq: {} vs {}",
            sum_sq,
            expected_sum_sq
        );
    }

    #[test]
    fn test_neon_sum_abs_deviations() {
        let values: Vec<f32> = (1..=100).map(|i| i as f32).collect();
        let median = 50.5;
        let sum = unsafe { sum_abs_deviations_neon(&values, median) };

        let expected: f32 = values.iter().map(|&v| (v - median).abs()).sum();

        assert!(
            (sum - expected).abs() < 0.1,
            "Sum abs dev: {} vs {}",
            sum,
            expected
        );
    }
}
