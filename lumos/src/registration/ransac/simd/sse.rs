//! SSE2 and AVX2 SIMD implementations for RANSAC operations.

#![allow(clippy::needless_range_loop)]

use glam::DVec2;
#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;

use crate::registration::transform::Transform;

/// Count inliers using AVX2 SIMD (processes 4 points at a time with f64).
///
/// # Safety
/// - Caller must ensure AVX2 is available.
/// - All slices must have the same length.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
pub unsafe fn count_inliers_avx2(
    ref_points: &[DVec2],
    target_points: &[DVec2],
    transform: &Transform,
    threshold_sq: f64,
) -> (Vec<usize>, f64) {
    unsafe {
        let len = ref_points.len();
        let mut inliers = Vec::with_capacity(len);
        let mut total_score = 0.0f64;

        // Extract transform components for vectorized computation
        // Transform: [a, b, c, d, e, f, g, h, 1] in row-major
        // x' = (a*x + b*y + c) / (g*x + h*y + 1)
        // y' = (d*x + e*y + f) / (g*x + h*y + 1)
        let t = transform.matrix.as_array();

        let a = _mm256_set1_pd(t[0]);
        let b = _mm256_set1_pd(t[1]);
        let c = _mm256_set1_pd(t[2]);
        let d = _mm256_set1_pd(t[3]);
        let e = _mm256_set1_pd(t[4]);
        let f = _mm256_set1_pd(t[5]);
        let g = _mm256_set1_pd(t[6]);
        let h = _mm256_set1_pd(t[7]);
        let one = _mm256_set1_pd(1.0);
        let thresh_sq_vec = _mm256_set1_pd(threshold_sq);

        let chunks = len / 4;

        for chunk in 0..chunks {
            let base = chunk * 4;

            // Load 4 reference points (x0,y0), (x1,y1), (x2,y2), (x3,y3)
            let r0 = ref_points[base];
            let r1 = ref_points[base + 1];
            let r2 = ref_points[base + 2];
            let r3 = ref_points[base + 3];

            let ref_x = _mm256_set_pd(r3.x, r2.x, r1.x, r0.x);
            let ref_y = _mm256_set_pd(r3.y, r2.y, r1.y, r0.y);

            // Load 4 target points
            let t0 = target_points[base];
            let t1 = target_points[base + 1];
            let t2 = target_points[base + 2];
            let t3 = target_points[base + 3];

            let tar_x = _mm256_set_pd(t3.x, t2.x, t1.x, t0.x);
            let tar_y = _mm256_set_pd(t3.y, t2.y, t1.y, t0.y);

            // Compute transformed x': (a*x + b*y + c) / (g*x + h*y + 1)
            let num_x = _mm256_add_pd(
                _mm256_add_pd(_mm256_mul_pd(a, ref_x), _mm256_mul_pd(b, ref_y)),
                c,
            );
            let num_y = _mm256_add_pd(
                _mm256_add_pd(_mm256_mul_pd(d, ref_x), _mm256_mul_pd(e, ref_y)),
                f,
            );
            let denom = _mm256_add_pd(
                _mm256_add_pd(_mm256_mul_pd(g, ref_x), _mm256_mul_pd(h, ref_y)),
                one,
            );

            let trans_x = _mm256_div_pd(num_x, denom);
            let trans_y = _mm256_div_pd(num_y, denom);

            // Compute squared distance: (trans_x - tar_x)² + (trans_y - tar_y)²
            let dx = _mm256_sub_pd(trans_x, tar_x);
            let dy = _mm256_sub_pd(trans_y, tar_y);
            let dist_sq = _mm256_add_pd(_mm256_mul_pd(dx, dx), _mm256_mul_pd(dy, dy));

            // Compare: dist_sq < threshold_sq
            let mask = _mm256_cmp_pd(dist_sq, thresh_sq_vec, _CMP_LT_OQ);
            let mask_bits = _mm256_movemask_pd(mask) as u8;

            // Compute scores: threshold_sq - dist_sq
            let score_contrib = _mm256_sub_pd(thresh_sq_vec, dist_sq);

            // Extract results
            let mut score_arr = [0.0f64; 4];
            _mm256_storeu_pd(score_arr.as_mut_ptr(), score_contrib);

            for i in 0..4 {
                if (mask_bits & (1 << i)) != 0 {
                    inliers.push(base + i);
                    total_score += score_arr[i];
                }
            }
        }

        // Handle remainder with scalar
        let remainder_start = chunks * 4;
        for i in remainder_start..len {
            let r = ref_points[i];
            let t = target_points[i];
            let p = transform.apply(r);
            let dist_sq = (p - t).length_squared();

            if dist_sq < threshold_sq {
                inliers.push(i);
                total_score += threshold_sq - dist_sq;
            }
        }

        (inliers, total_score)
    }
}

/// Count inliers using SSE2 SIMD (processes 2 points at a time with f64).
///
/// # Safety
/// - Caller must ensure SSE2 is available.
/// - All slices must have the same length.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse2")]
pub unsafe fn count_inliers_sse2(
    ref_points: &[DVec2],
    target_points: &[DVec2],
    transform: &Transform,
    threshold_sq: f64,
) -> (Vec<usize>, f64) {
    unsafe {
        let len = ref_points.len();
        let mut inliers = Vec::with_capacity(len);
        let mut total_score = 0.0f64;

        let t = transform.matrix.as_array();

        let a = _mm_set1_pd(t[0]);
        let b = _mm_set1_pd(t[1]);
        let c = _mm_set1_pd(t[2]);
        let d = _mm_set1_pd(t[3]);
        let e = _mm_set1_pd(t[4]);
        let f = _mm_set1_pd(t[5]);
        let g = _mm_set1_pd(t[6]);
        let h = _mm_set1_pd(t[7]);
        let one = _mm_set1_pd(1.0);
        let thresh_sq_vec = _mm_set1_pd(threshold_sq);

        let chunks = len / 2;

        for chunk in 0..chunks {
            let base = chunk * 2;

            // Load 2 reference points
            let r0 = ref_points[base];
            let r1 = ref_points[base + 1];

            let ref_x = _mm_set_pd(r1.x, r0.x);
            let ref_y = _mm_set_pd(r1.y, r0.y);

            // Load 2 target points
            let t0 = target_points[base];
            let t1 = target_points[base + 1];

            let tar_x = _mm_set_pd(t1.x, t0.x);
            let tar_y = _mm_set_pd(t1.y, t0.y);

            // Compute transformed coordinates
            let num_x = _mm_add_pd(_mm_add_pd(_mm_mul_pd(a, ref_x), _mm_mul_pd(b, ref_y)), c);
            let num_y = _mm_add_pd(_mm_add_pd(_mm_mul_pd(d, ref_x), _mm_mul_pd(e, ref_y)), f);
            let denom = _mm_add_pd(_mm_add_pd(_mm_mul_pd(g, ref_x), _mm_mul_pd(h, ref_y)), one);

            let trans_x = _mm_div_pd(num_x, denom);
            let trans_y = _mm_div_pd(num_y, denom);

            // Compute squared distance
            let dx = _mm_sub_pd(trans_x, tar_x);
            let dy = _mm_sub_pd(trans_y, tar_y);
            let dist_sq = _mm_add_pd(_mm_mul_pd(dx, dx), _mm_mul_pd(dy, dy));

            // Compare: dist_sq < threshold_sq
            let mask = _mm_cmplt_pd(dist_sq, thresh_sq_vec);
            let mask_bits = _mm_movemask_pd(mask) as u8;

            // Compute scores: threshold_sq - dist_sq
            let score_contrib = _mm_sub_pd(thresh_sq_vec, dist_sq);

            // Extract results
            let mut score_arr = [0.0f64; 2];
            _mm_storeu_pd(score_arr.as_mut_ptr(), score_contrib);

            for i in 0..2 {
                if (mask_bits & (1 << i)) != 0 {
                    inliers.push(base + i);
                    total_score += score_arr[i];
                }
            }
        }

        // Handle remainder
        let remainder_start = chunks * 2;
        for i in remainder_start..len {
            let r = ref_points[i];
            let t = target_points[i];
            let p = transform.apply(r);
            let dist_sq = (p - t).length_squared();

            if dist_sq < threshold_sq {
                inliers.push(i);
                total_score += threshold_sq - dist_sq;
            }
        }

        (inliers, total_score)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_arch = "x86_64")]
    use common::cpu_features;

    fn count_inliers_scalar(
        ref_points: &[DVec2],
        target_points: &[DVec2],
        transform: &Transform,
        threshold_sq: f64,
    ) -> (Vec<usize>, f64) {
        let mut inliers = Vec::new();
        let mut score = 0.0f64;

        for (i, (r, t)) in ref_points.iter().zip(target_points.iter()).enumerate() {
            let p = transform.apply(*r);
            let dist_sq = (p - *t).length_squared();

            if dist_sq < threshold_sq {
                inliers.push(i);
                score += threshold_sq - dist_sq;
            }
        }

        (inliers, score)
    }

    #[test]
    #[cfg(target_arch = "x86_64")]
    fn test_avx2_count_inliers() {
        if !cpu_features::has_avx2() {
            eprintln!("Skipping AVX2 test - not available");
            return;
        }

        let transform = Transform::translation(DVec2::new(10.0, 5.0));
        let ref_points: Vec<DVec2> = (0..20)
            .map(|i| DVec2::new(i as f64 * 5.0, i as f64 * 3.0))
            .collect();
        let target_points: Vec<DVec2> = ref_points
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let t = transform.apply(*p);
                if i % 4 == 0 {
                    t + DVec2::new(100.0, 0.0) // outlier
                } else {
                    t + DVec2::new(0.1, -0.1)
                }
            })
            .collect();
        let threshold_sq = 4.0; // threshold = 2.0

        let (inliers_avx2, score_avx2) =
            unsafe { count_inliers_avx2(&ref_points, &target_points, &transform, threshold_sq) };
        let (inliers_scalar, score_scalar) =
            count_inliers_scalar(&ref_points, &target_points, &transform, threshold_sq);

        assert_eq!(inliers_avx2, inliers_scalar, "Inliers mismatch");
        assert!((score_avx2 - score_scalar).abs() < 1e-6, "Score mismatch");
    }

    #[test]
    #[cfg(target_arch = "x86_64")]
    fn test_sse2_count_inliers() {
        if !cpu_features::has_sse4_1() {
            eprintln!("Skipping SSE2 test - not available");
            return;
        }

        let transform = Transform::similarity(DVec2::new(5.0, 10.0), 0.1, 1.1);
        let ref_points: Vec<DVec2> = (0..15)
            .map(|i| DVec2::new(i as f64 * 7.0, i as f64 * 4.0))
            .collect();
        let target_points: Vec<DVec2> = ref_points
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let t = transform.apply(*p);
                if i % 3 == 0 {
                    t + DVec2::new(50.0, 50.0)
                } else {
                    t + DVec2::new(0.2, -0.15)
                }
            })
            .collect();
        let threshold_sq = 1.5 * 1.5; // threshold = 1.5

        let (inliers_sse, score_sse) =
            unsafe { count_inliers_sse2(&ref_points, &target_points, &transform, threshold_sq) };
        let (inliers_scalar, score_scalar) =
            count_inliers_scalar(&ref_points, &target_points, &transform, threshold_sq);

        assert_eq!(inliers_sse, inliers_scalar, "Inliers mismatch");
        assert!((score_sse - score_scalar).abs() < 1e-6, "Score mismatch");
    }
}
