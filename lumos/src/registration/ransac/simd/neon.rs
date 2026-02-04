//! NEON SIMD implementations for RANSAC operations (aarch64).

#![allow(clippy::needless_range_loop)]

use glam::DVec2;
#[cfg(target_arch = "aarch64")]
use std::arch::aarch64::*;

use crate::registration::transform::Transform;

/// Count inliers using NEON SIMD (processes 2 points at a time with f64).
///
/// # Safety
/// - Caller must ensure NEON is available (always true on aarch64).
/// - All slices must have the same length.
#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
pub unsafe fn count_inliers_neon(
    ref_points: &[DVec2],
    target_points: &[DVec2],
    transform: &Transform,
    threshold: f64,
) -> (Vec<usize>, usize) {
    unsafe {
        let len = ref_points.len();
        let threshold_sq = threshold * threshold;
        let mut inliers = Vec::with_capacity(len);
        let mut total_score = 0usize;

        let t = transform.matrix.as_array();

        let a = vdupq_n_f64(t[0]);
        let b = vdupq_n_f64(t[1]);
        let c = vdupq_n_f64(t[2]);
        let d = vdupq_n_f64(t[3]);
        let e = vdupq_n_f64(t[4]);
        let f = vdupq_n_f64(t[5]);
        let g = vdupq_n_f64(t[6]);
        let h = vdupq_n_f64(t[7]);
        let one = vdupq_n_f64(1.0);
        let thresh_sq_vec = vdupq_n_f64(threshold_sq);
        let score_scale = vdupq_n_f64(1000.0);

        let chunks = len / 2;

        for chunk in 0..chunks {
            let base = chunk * 2;

            // Load 2 reference points
            let r0 = ref_points[base];
            let r1 = ref_points[base + 1];

            let ref_x = vld1q_f64([r0.x, r1.x].as_ptr());
            let ref_y = vld1q_f64([r0.y, r1.y].as_ptr());

            // Load 2 target points
            let t0 = target_points[base];
            let t1 = target_points[base + 1];

            let tar_x = vld1q_f64([t0.x, t1.x].as_ptr());
            let tar_y = vld1q_f64([t0.y, t1.y].as_ptr());

            // Compute transformed x': (a*x + b*y + c) / (g*x + h*y + 1)
            let ax = vmulq_f64(a, ref_x);
            let by = vmulq_f64(b, ref_y);
            let num_x = vaddq_f64(vaddq_f64(ax, by), c);

            let dx_coef = vmulq_f64(d, ref_x);
            let ey = vmulq_f64(e, ref_y);
            let num_y = vaddq_f64(vaddq_f64(dx_coef, ey), f);

            let gx = vmulq_f64(g, ref_x);
            let hy = vmulq_f64(h, ref_y);
            let denom = vaddq_f64(vaddq_f64(gx, hy), one);

            let trans_x = vdivq_f64(num_x, denom);
            let trans_y = vdivq_f64(num_y, denom);

            // Compute squared distance
            let dx = vsubq_f64(trans_x, tar_x);
            let dy = vsubq_f64(trans_y, tar_y);
            let dx_sq = vmulq_f64(dx, dx);
            let dy_sq = vmulq_f64(dy, dy);
            let dist_sq = vaddq_f64(dx_sq, dy_sq);

            // Compare: dist_sq < threshold_sq
            let mask = vcltq_f64(dist_sq, thresh_sq_vec);

            // Compute scores
            let score_contrib = vmulq_f64(vsubq_f64(thresh_sq_vec, dist_sq), score_scale);

            // Extract results
            let mut dist_sq_arr = [0.0f64; 2];
            let mut score_arr = [0.0f64; 2];
            let mut mask_arr = [0u64; 2];
            vst1q_f64(dist_sq_arr.as_mut_ptr(), dist_sq);
            vst1q_f64(score_arr.as_mut_ptr(), score_contrib);
            vst1q_u64(mask_arr.as_mut_ptr(), mask);

            for i in 0..2 {
                if mask_arr[i] != 0 {
                    inliers.push(base + i);
                    total_score += score_arr[i] as usize;
                }
            }
        }

        // Handle remainder
        let remainder_start = chunks * 2;
        for i in remainder_start..len {
            let r = ref_points[i];
            let t = target_points[i];
            let p = transform.apply_vec(r);
            let dist_sq = (p - t).length_squared();

            if dist_sq < threshold_sq {
                inliers.push(i);
                total_score += ((threshold_sq - dist_sq) * 1000.0) as usize;
            }
        }

        (inliers, total_score)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count_inliers_scalar(
        ref_points: &[DVec2],
        target_points: &[DVec2],
        transform: &Transform,
        threshold: f64,
    ) -> (Vec<usize>, usize) {
        let threshold_sq = threshold * threshold;
        let mut inliers = Vec::new();
        let mut score = 0usize;

        for (i, (r, t)) in ref_points.iter().zip(target_points.iter()).enumerate() {
            let p = transform.apply_vec(*r);
            let dist_sq = (p - *t).length_squared();

            if dist_sq < threshold_sq {
                inliers.push(i);
                score += ((threshold_sq - dist_sq) * 1000.0) as usize;
            }
        }

        (inliers, score)
    }

    #[test]
    #[cfg(target_arch = "aarch64")]
    fn test_neon_count_inliers() {
        let transform = Transform::translation(10.0, 5.0);
        let ref_points: Vec<DVec2> = (0..20)
            .map(|i| DVec2::new(i as f64 * 5.0, i as f64 * 3.0))
            .collect();
        let target_points: Vec<DVec2> = ref_points
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let t = transform.apply_vec(*p);
                if i % 4 == 0 {
                    t + DVec2::new(100.0, 0.0)
                } else {
                    t + DVec2::new(0.1, -0.1)
                }
            })
            .collect();
        let threshold = 2.0;

        let (inliers_neon, score_neon) =
            unsafe { count_inliers_neon(&ref_points, &target_points, &transform, threshold) };
        let (inliers_scalar, score_scalar) =
            count_inliers_scalar(&ref_points, &target_points, &transform, threshold);

        assert_eq!(inliers_neon, inliers_scalar, "Inliers mismatch");
        assert_eq!(score_neon, score_scalar, "Score mismatch");
    }
}
