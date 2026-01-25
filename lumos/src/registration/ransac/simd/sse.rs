//! SSE2 and AVX2 SIMD implementations for RANSAC operations.

#![allow(clippy::needless_range_loop)]

#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;

use crate::registration::types::TransformMatrix;

/// Count inliers using AVX2 SIMD (processes 4 points at a time with f64).
///
/// # Safety
/// - Caller must ensure AVX2 is available.
/// - All slices must have the same length.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
pub unsafe fn count_inliers_avx2(
    ref_points: &[(f64, f64)],
    target_points: &[(f64, f64)],
    transform: &TransformMatrix,
    threshold: f64,
) -> (Vec<usize>, usize) {
    unsafe {
        let len = ref_points.len();
        let threshold_sq = threshold * threshold;
        let mut inliers = Vec::with_capacity(len);
        let mut total_score = 0usize;

        // Extract transform components for vectorized computation
        // Transform: [a, b, c, d, e, f, g, h, 1] in row-major
        // x' = (a*x + b*y + c) / (g*x + h*y + 1)
        // y' = (d*x + e*y + f) / (g*x + h*y + 1)
        let t = &transform.data;

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
        let score_scale = _mm256_set1_pd(1000.0);

        let chunks = len / 4;

        for chunk in 0..chunks {
            let base = chunk * 4;

            // Load 4 reference points (x0,y0), (x1,y1), (x2,y2), (x3,y3)
            let rx0 = ref_points[base].0;
            let ry0 = ref_points[base].1;
            let rx1 = ref_points[base + 1].0;
            let ry1 = ref_points[base + 1].1;
            let rx2 = ref_points[base + 2].0;
            let ry2 = ref_points[base + 2].1;
            let rx3 = ref_points[base + 3].0;
            let ry3 = ref_points[base + 3].1;

            let ref_x = _mm256_set_pd(rx3, rx2, rx1, rx0);
            let ref_y = _mm256_set_pd(ry3, ry2, ry1, ry0);

            // Load 4 target points
            let tx0 = target_points[base].0;
            let ty0 = target_points[base].1;
            let tx1 = target_points[base + 1].0;
            let ty1 = target_points[base + 1].1;
            let tx2 = target_points[base + 2].0;
            let ty2 = target_points[base + 2].1;
            let tx3 = target_points[base + 3].0;
            let ty3 = target_points[base + 3].1;

            let tar_x = _mm256_set_pd(tx3, tx2, tx1, tx0);
            let tar_y = _mm256_set_pd(ty3, ty2, ty1, ty0);

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

            // Compute scores: (threshold_sq - dist_sq) * 1000
            let score_contrib = _mm256_mul_pd(_mm256_sub_pd(thresh_sq_vec, dist_sq), score_scale);

            // Extract results
            let mut dist_sq_arr = [0.0f64; 4];
            let mut score_arr = [0.0f64; 4];
            _mm256_storeu_pd(dist_sq_arr.as_mut_ptr(), dist_sq);
            _mm256_storeu_pd(score_arr.as_mut_ptr(), score_contrib);

            for i in 0..4 {
                if (mask_bits & (1 << i)) != 0 {
                    inliers.push(base + i);
                    total_score += score_arr[i] as usize;
                }
            }
        }

        // Handle remainder with scalar
        let remainder_start = chunks * 4;
        for i in remainder_start..len {
            let (rx, ry) = ref_points[i];
            let (tx, ty) = target_points[i];
            let (px, py) = transform.apply(rx, ry);
            let dist_sq = (px - tx).powi(2) + (py - ty).powi(2);

            if dist_sq < threshold_sq {
                inliers.push(i);
                total_score += ((threshold_sq - dist_sq) * 1000.0) as usize;
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
    ref_points: &[(f64, f64)],
    target_points: &[(f64, f64)],
    transform: &TransformMatrix,
    threshold: f64,
) -> (Vec<usize>, usize) {
    unsafe {
        let len = ref_points.len();
        let threshold_sq = threshold * threshold;
        let mut inliers = Vec::with_capacity(len);
        let mut total_score = 0usize;

        let t = &transform.data;

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
        let score_scale = _mm_set1_pd(1000.0);

        let chunks = len / 2;

        for chunk in 0..chunks {
            let base = chunk * 2;

            // Load 2 reference points
            let rx0 = ref_points[base].0;
            let ry0 = ref_points[base].1;
            let rx1 = ref_points[base + 1].0;
            let ry1 = ref_points[base + 1].1;

            let ref_x = _mm_set_pd(rx1, rx0);
            let ref_y = _mm_set_pd(ry1, ry0);

            // Load 2 target points
            let tx0 = target_points[base].0;
            let ty0 = target_points[base].1;
            let tx1 = target_points[base + 1].0;
            let ty1 = target_points[base + 1].1;

            let tar_x = _mm_set_pd(tx1, tx0);
            let tar_y = _mm_set_pd(ty1, ty0);

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

            // Compute scores
            let score_contrib = _mm_mul_pd(_mm_sub_pd(thresh_sq_vec, dist_sq), score_scale);

            // Extract results
            let mut dist_sq_arr = [0.0f64; 2];
            let mut score_arr = [0.0f64; 2];
            _mm_storeu_pd(dist_sq_arr.as_mut_ptr(), dist_sq);
            _mm_storeu_pd(score_arr.as_mut_ptr(), score_contrib);

            for i in 0..2 {
                if (mask_bits & (1 << i)) != 0 {
                    inliers.push(base + i);
                    total_score += score_arr[i] as usize;
                }
            }
        }

        // Handle remainder
        let remainder_start = chunks * 2;
        for i in remainder_start..len {
            let (rx, ry) = ref_points[i];
            let (tx, ty) = target_points[i];
            let (px, py) = transform.apply(rx, ry);
            let dist_sq = (px - tx).powi(2) + (py - ty).powi(2);

            if dist_sq < threshold_sq {
                inliers.push(i);
                total_score += ((threshold_sq - dist_sq) * 1000.0) as usize;
            }
        }

        (inliers, total_score)
    }
}

/// Compute residuals using AVX2 SIMD.
///
/// # Safety
/// - Caller must ensure AVX2 is available.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
pub unsafe fn compute_residuals_avx2(
    ref_points: &[(f64, f64)],
    target_points: &[(f64, f64)],
    transform: &TransformMatrix,
) -> Vec<f64> {
    unsafe {
        let len = ref_points.len();
        let mut residuals = vec![0.0f64; len];

        let t = &transform.data;

        let a = _mm256_set1_pd(t[0]);
        let b = _mm256_set1_pd(t[1]);
        let c = _mm256_set1_pd(t[2]);
        let d = _mm256_set1_pd(t[3]);
        let e = _mm256_set1_pd(t[4]);
        let f = _mm256_set1_pd(t[5]);
        let g = _mm256_set1_pd(t[6]);
        let h = _mm256_set1_pd(t[7]);
        let one = _mm256_set1_pd(1.0);

        let chunks = len / 4;

        for chunk in 0..chunks {
            let base = chunk * 4;

            // Load 4 reference points
            let rx0 = ref_points[base].0;
            let ry0 = ref_points[base].1;
            let rx1 = ref_points[base + 1].0;
            let ry1 = ref_points[base + 1].1;
            let rx2 = ref_points[base + 2].0;
            let ry2 = ref_points[base + 2].1;
            let rx3 = ref_points[base + 3].0;
            let ry3 = ref_points[base + 3].1;

            let ref_x = _mm256_set_pd(rx3, rx2, rx1, rx0);
            let ref_y = _mm256_set_pd(ry3, ry2, ry1, ry0);

            // Load 4 target points
            let tx0 = target_points[base].0;
            let ty0 = target_points[base].1;
            let tx1 = target_points[base + 1].0;
            let ty1 = target_points[base + 1].1;
            let tx2 = target_points[base + 2].0;
            let ty2 = target_points[base + 2].1;
            let tx3 = target_points[base + 3].0;
            let ty3 = target_points[base + 3].1;

            let tar_x = _mm256_set_pd(tx3, tx2, tx1, tx0);
            let tar_y = _mm256_set_pd(ty3, ty2, ty1, ty0);

            // Compute transformed coordinates
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

            // Compute distance: sqrt((trans_x - tar_x)² + (trans_y - tar_y)²)
            let dx = _mm256_sub_pd(trans_x, tar_x);
            let dy = _mm256_sub_pd(trans_y, tar_y);
            let dist_sq = _mm256_add_pd(_mm256_mul_pd(dx, dx), _mm256_mul_pd(dy, dy));
            let dist = _mm256_sqrt_pd(dist_sq);

            // Store results
            _mm256_storeu_pd(residuals.as_mut_ptr().add(base), dist);
        }

        // Handle remainder
        let remainder_start = chunks * 4;
        for i in remainder_start..len {
            let (rx, ry) = ref_points[i];
            let (tx, ty) = target_points[i];
            let (px, py) = transform.apply(rx, ry);
            residuals[i] = ((px - tx).powi(2) + (py - ty).powi(2)).sqrt();
        }

        residuals
    }
}

/// Compute residuals using SSE2 SIMD.
///
/// # Safety
/// - Caller must ensure SSE2 is available.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse2")]
pub unsafe fn compute_residuals_sse2(
    ref_points: &[(f64, f64)],
    target_points: &[(f64, f64)],
    transform: &TransformMatrix,
) -> Vec<f64> {
    unsafe {
        let len = ref_points.len();
        let mut residuals = vec![0.0f64; len];

        let t = &transform.data;

        let a = _mm_set1_pd(t[0]);
        let b = _mm_set1_pd(t[1]);
        let c = _mm_set1_pd(t[2]);
        let d = _mm_set1_pd(t[3]);
        let e = _mm_set1_pd(t[4]);
        let f = _mm_set1_pd(t[5]);
        let g = _mm_set1_pd(t[6]);
        let h = _mm_set1_pd(t[7]);
        let one = _mm_set1_pd(1.0);

        let chunks = len / 2;

        for chunk in 0..chunks {
            let base = chunk * 2;

            // Load 2 reference points
            let rx0 = ref_points[base].0;
            let ry0 = ref_points[base].1;
            let rx1 = ref_points[base + 1].0;
            let ry1 = ref_points[base + 1].1;

            let ref_x = _mm_set_pd(rx1, rx0);
            let ref_y = _mm_set_pd(ry1, ry0);

            // Load 2 target points
            let tx0 = target_points[base].0;
            let ty0 = target_points[base].1;
            let tx1 = target_points[base + 1].0;
            let ty1 = target_points[base + 1].1;

            let tar_x = _mm_set_pd(tx1, tx0);
            let tar_y = _mm_set_pd(ty1, ty0);

            // Compute transformed coordinates
            let num_x = _mm_add_pd(_mm_add_pd(_mm_mul_pd(a, ref_x), _mm_mul_pd(b, ref_y)), c);
            let num_y = _mm_add_pd(_mm_add_pd(_mm_mul_pd(d, ref_x), _mm_mul_pd(e, ref_y)), f);
            let denom = _mm_add_pd(_mm_add_pd(_mm_mul_pd(g, ref_x), _mm_mul_pd(h, ref_y)), one);

            let trans_x = _mm_div_pd(num_x, denom);
            let trans_y = _mm_div_pd(num_y, denom);

            // Compute distance
            let dx = _mm_sub_pd(trans_x, tar_x);
            let dy = _mm_sub_pd(trans_y, tar_y);
            let dist_sq = _mm_add_pd(_mm_mul_pd(dx, dx), _mm_mul_pd(dy, dy));
            let dist = _mm_sqrt_pd(dist_sq);

            // Store results
            _mm_storeu_pd(residuals.as_mut_ptr().add(base), dist);
        }

        // Handle remainder
        let remainder_start = chunks * 2;
        for i in remainder_start..len {
            let (rx, ry) = ref_points[i];
            let (tx, ty) = target_points[i];
            let (px, py) = transform.apply(rx, ry);
            residuals[i] = ((px - tx).powi(2) + (py - ty).powi(2)).sqrt();
        }

        residuals
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_arch = "x86_64")]
    use crate::common::cpu_features;

    fn count_inliers_scalar(
        ref_points: &[(f64, f64)],
        target_points: &[(f64, f64)],
        transform: &TransformMatrix,
        threshold: f64,
    ) -> (Vec<usize>, usize) {
        let threshold_sq = threshold * threshold;
        let mut inliers = Vec::new();
        let mut score = 0usize;

        for (i, (&(rx, ry), &(tx, ty))) in ref_points.iter().zip(target_points.iter()).enumerate() {
            let (px, py) = transform.apply(rx, ry);
            let dist_sq = (px - tx).powi(2) + (py - ty).powi(2);

            if dist_sq < threshold_sq {
                inliers.push(i);
                score += ((threshold_sq - dist_sq) * 1000.0) as usize;
            }
        }

        (inliers, score)
    }

    fn compute_residuals_scalar(
        ref_points: &[(f64, f64)],
        target_points: &[(f64, f64)],
        transform: &TransformMatrix,
    ) -> Vec<f64> {
        ref_points
            .iter()
            .zip(target_points.iter())
            .map(|(&(rx, ry), &(tx, ty))| {
                let (px, py) = transform.apply(rx, ry);
                ((px - tx).powi(2) + (py - ty).powi(2)).sqrt()
            })
            .collect()
    }

    #[test]
    #[cfg(target_arch = "x86_64")]
    fn test_avx2_count_inliers() {
        if !cpu_features::has_avx2() {
            eprintln!("Skipping AVX2 test - not available");
            return;
        }

        let transform = TransformMatrix::translation(10.0, 5.0);
        let ref_points: Vec<(f64, f64)> =
            (0..20).map(|i| (i as f64 * 5.0, i as f64 * 3.0)).collect();
        let target_points: Vec<(f64, f64)> = ref_points
            .iter()
            .enumerate()
            .map(|(i, &(x, y))| {
                let (tx, ty) = transform.apply(x, y);
                if i % 4 == 0 {
                    (tx + 100.0, ty) // outlier
                } else {
                    (tx + 0.1, ty - 0.1)
                }
            })
            .collect();
        let threshold = 2.0;

        let (inliers_avx2, score_avx2) =
            unsafe { count_inliers_avx2(&ref_points, &target_points, &transform, threshold) };
        let (inliers_scalar, score_scalar) =
            count_inliers_scalar(&ref_points, &target_points, &transform, threshold);

        assert_eq!(inliers_avx2, inliers_scalar, "Inliers mismatch");
        assert_eq!(score_avx2, score_scalar, "Score mismatch");
    }

    #[test]
    #[cfg(target_arch = "x86_64")]
    fn test_sse2_count_inliers() {
        if !cpu_features::has_sse4_1() {
            eprintln!("Skipping SSE2 test - not available");
            return;
        }

        let transform = TransformMatrix::similarity(5.0, 10.0, 0.1, 1.1);
        let ref_points: Vec<(f64, f64)> =
            (0..15).map(|i| (i as f64 * 7.0, i as f64 * 4.0)).collect();
        let target_points: Vec<(f64, f64)> = ref_points
            .iter()
            .enumerate()
            .map(|(i, &(x, y))| {
                let (tx, ty) = transform.apply(x, y);
                if i % 3 == 0 {
                    (tx + 50.0, ty + 50.0)
                } else {
                    (tx + 0.2, ty - 0.15)
                }
            })
            .collect();
        let threshold = 1.5;

        let (inliers_sse, score_sse) =
            unsafe { count_inliers_sse2(&ref_points, &target_points, &transform, threshold) };
        let (inliers_scalar, score_scalar) =
            count_inliers_scalar(&ref_points, &target_points, &transform, threshold);

        assert_eq!(inliers_sse, inliers_scalar, "Inliers mismatch");
        assert_eq!(score_sse, score_scalar, "Score mismatch");
    }

    #[test]
    #[cfg(target_arch = "x86_64")]
    fn test_avx2_compute_residuals() {
        if !cpu_features::has_avx2() {
            eprintln!("Skipping AVX2 residuals test - not available");
            return;
        }

        let transform = TransformMatrix::affine([1.1, 0.05, 10.0, -0.05, 0.95, 5.0]);
        let ref_points: Vec<(f64, f64)> =
            (0..25).map(|i| (i as f64 * 4.0, i as f64 * 2.5)).collect();
        let target_points: Vec<(f64, f64)> = ref_points
            .iter()
            .map(|&(x, y)| {
                let (tx, ty) = transform.apply(x, y);
                (tx + 0.3, ty - 0.2)
            })
            .collect();

        let residuals_avx2 =
            unsafe { compute_residuals_avx2(&ref_points, &target_points, &transform) };
        let residuals_scalar = compute_residuals_scalar(&ref_points, &target_points, &transform);

        for (i, (a, s)) in residuals_avx2
            .iter()
            .zip(residuals_scalar.iter())
            .enumerate()
        {
            assert!(
                (a - s).abs() < 1e-10,
                "Index {}: AVX2 {} vs Scalar {}",
                i,
                a,
                s
            );
        }
    }

    #[test]
    #[cfg(target_arch = "x86_64")]
    fn test_sse2_compute_residuals() {
        if !cpu_features::has_sse4_1() {
            eprintln!("Skipping SSE2 residuals test - not available");
            return;
        }

        let transform = TransformMatrix::euclidean(15.0, -8.0, 0.2);
        let ref_points: Vec<(f64, f64)> = (0..17)
            .map(|i| (i as f64 * 6.0 + 1.0, i as f64 * 3.5))
            .collect();
        let target_points: Vec<(f64, f64)> = ref_points
            .iter()
            .map(|&(x, y)| {
                let (tx, ty) = transform.apply(x, y);
                (tx - 0.15, ty + 0.25)
            })
            .collect();

        let residuals_sse =
            unsafe { compute_residuals_sse2(&ref_points, &target_points, &transform) };
        let residuals_scalar = compute_residuals_scalar(&ref_points, &target_points, &transform);

        for (i, (a, s)) in residuals_sse
            .iter()
            .zip(residuals_scalar.iter())
            .enumerate()
        {
            assert!(
                (a - s).abs() < 1e-10,
                "Index {}: SSE2 {} vs Scalar {}",
                i,
                a,
                s
            );
        }
    }
}
