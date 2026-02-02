//! SIMD-accelerated RANSAC utilities.
//!
//! This module provides runtime dispatch to the best available SIMD implementation:
//! - AVX2/SSE4.1 on x86_64
//! - NEON on aarch64
//! - Scalar fallback on other platforms

#[cfg(target_arch = "x86_64")]
use common::cpu_features;

#[cfg(target_arch = "x86_64")]
pub mod sse;

#[cfg(target_arch = "aarch64")]
pub mod neon;

use crate::registration::transform::TransformMatrix;

/// Count inliers and compute score using SIMD acceleration.
///
/// For each point pair, computes: dist² = (transform(ref) - target)²
/// Returns indices where dist² < threshold² and a weighted score.
///
/// This function dispatches to the best available SIMD implementation at runtime.
#[inline]
pub(crate) fn count_inliers_simd(
    ref_points: &[(f64, f64)],
    target_points: &[(f64, f64)],
    transform: &TransformMatrix,
    threshold: f64,
) -> (Vec<usize>, usize) {
    let len = ref_points.len();

    if len == 0 {
        return (Vec::new(), 0);
    }

    #[cfg(target_arch = "x86_64")]
    {
        if len >= 4 && cpu_features::has_avx2() {
            return unsafe {
                sse::count_inliers_avx2(ref_points, target_points, transform, threshold)
            };
        }
        if len >= 2 && cpu_features::has_sse4_1() {
            return unsafe {
                sse::count_inliers_sse2(ref_points, target_points, transform, threshold)
            };
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        if len >= 2 {
            return unsafe {
                neon::count_inliers_neon(ref_points, target_points, transform, threshold)
            };
        }
    }

    // Scalar fallback
    count_inliers_scalar(ref_points, target_points, transform, threshold)
}

/// Scalar implementation of inlier counting.
#[cfg(test)]
pub(crate) fn count_inliers_scalar(
    ref_points: &[(f64, f64)],
    target_points: &[(f64, f64)],
    transform: &TransformMatrix,
    threshold: f64,
) -> (Vec<usize>, usize) {
    count_inliers_scalar_impl(ref_points, target_points, transform, threshold)
}

#[cfg(not(test))]
fn count_inliers_scalar(
    ref_points: &[(f64, f64)],
    target_points: &[(f64, f64)],
    transform: &TransformMatrix,
    threshold: f64,
) -> (Vec<usize>, usize) {
    count_inliers_scalar_impl(ref_points, target_points, transform, threshold)
}

#[inline]
fn count_inliers_scalar_impl(
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
            // Use inverse distance as score contribution
            score += ((threshold_sq - dist_sq) * 1000.0) as usize;
        }
    }

    (inliers, score)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_transform() -> TransformMatrix {
        // Simple translation: shift by (10, 5)
        TransformMatrix::translation(10.0, 5.0)
    }

    fn create_similarity_transform() -> TransformMatrix {
        // Rotation of 30 degrees, scale 1.5, translation (10, 20)
        TransformMatrix::similarity(10.0, 20.0, std::f64::consts::PI / 6.0, 1.5)
    }

    #[test]
    fn test_count_inliers_simd_basic() {
        let ref_points = vec![(0.0, 0.0), (10.0, 10.0), (20.0, 20.0), (30.0, 30.0)];
        // Target points after translation (10, 5)
        let target_points = vec![(10.0, 5.0), (20.0, 15.0), (30.0, 25.0), (40.0, 35.0)];
        let transform = create_test_transform();
        let threshold = 1.0;

        let (inliers, score) =
            count_inliers_simd(&ref_points, &target_points, &transform, threshold);
        let (inliers_scalar, score_scalar) =
            count_inliers_scalar(&ref_points, &target_points, &transform, threshold);

        assert_eq!(inliers, inliers_scalar);
        assert_eq!(score, score_scalar);
        assert_eq!(inliers.len(), 4); // All should be inliers (exact match)
    }

    #[test]
    fn test_count_inliers_simd_with_outliers() {
        let ref_points = vec![(0.0, 0.0), (10.0, 10.0), (20.0, 20.0), (30.0, 30.0)];
        // Some outliers
        let target_points = vec![
            (10.0, 5.0),    // inlier
            (25.0, 25.0),   // outlier (distance > threshold)
            (30.0, 25.0),   // inlier
            (100.0, 100.0), // outlier
        ];
        let transform = create_test_transform();
        let threshold = 2.0;

        let (inliers, _) = count_inliers_simd(&ref_points, &target_points, &transform, threshold);
        let (inliers_scalar, _) =
            count_inliers_scalar(&ref_points, &target_points, &transform, threshold);

        assert_eq!(inliers, inliers_scalar);
        assert_eq!(inliers.len(), 2); // Only indices 0 and 2
        assert!(inliers.contains(&0));
        assert!(inliers.contains(&2));
    }

    #[test]
    fn test_count_inliers_simd_empty() {
        let ref_points: Vec<(f64, f64)> = Vec::new();
        let target_points: Vec<(f64, f64)> = Vec::new();
        let transform = create_test_transform();

        let (inliers, score) = count_inliers_simd(&ref_points, &target_points, &transform, 1.0);
        assert!(inliers.is_empty());
        assert_eq!(score, 0);
    }

    #[test]
    fn test_count_inliers_simd_matches_scalar_various_sizes() {
        let transform = create_similarity_transform();
        let threshold = 2.0;

        for size in [1, 2, 3, 4, 5, 7, 8, 9, 15, 16, 17, 32, 64, 100] {
            let ref_points: Vec<(f64, f64)> = (0..size)
                .map(|i| (i as f64 * 10.0, i as f64 * 5.0))
                .collect();

            // Create target points with some transformation applied
            let target_points: Vec<(f64, f64)> = ref_points
                .iter()
                .enumerate()
                .map(|(i, &(x, y))| {
                    let (tx, ty) = transform.apply(x, y);
                    // Add some noise/outliers
                    if i % 5 == 0 {
                        (tx + 100.0, ty + 100.0) // outlier
                    } else {
                        (tx + 0.1, ty - 0.1) // small noise
                    }
                })
                .collect();

            let (inliers_simd, score_simd) =
                count_inliers_simd(&ref_points, &target_points, &transform, threshold);
            let (inliers_scalar, score_scalar) =
                count_inliers_scalar(&ref_points, &target_points, &transform, threshold);

            assert_eq!(
                inliers_simd, inliers_scalar,
                "Size {}: inliers mismatch",
                size
            );
            assert_eq!(score_simd, score_scalar, "Size {}: score mismatch", size);
        }
    }

    #[test]
    fn test_count_inliers_simd_score_calculation() {
        // Test that score is computed correctly
        let ref_points = vec![(0.0, 0.0), (10.0, 0.0)];
        let target_points = vec![(10.0, 5.0), (20.0, 5.0)]; // Exact match after translation
        let transform = create_test_transform();
        let threshold = 2.0;

        let (inliers, score) =
            count_inliers_simd(&ref_points, &target_points, &transform, threshold);
        let (_, score_scalar) =
            count_inliers_scalar(&ref_points, &target_points, &transform, threshold);

        assert_eq!(inliers.len(), 2);
        assert_eq!(score, score_scalar);
        // Score should be 2 * threshold² * 1000 = 2 * 4 * 1000 = 8000
        assert_eq!(score, 8000);
    }
}
