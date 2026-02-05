//! SIMD-accelerated RANSAC utilities.
//!
//! This module provides runtime dispatch to the best available SIMD implementation:
//! - AVX2/SSE4.1 on x86_64
//! - NEON on aarch64
//! - Scalar fallback on other platforms

#[cfg(target_arch = "x86_64")]
use common::cpu_features;
use glam::DVec2;

#[cfg(target_arch = "x86_64")]
pub mod sse;

#[cfg(target_arch = "aarch64")]
pub mod neon;

#[cfg(test)]
mod bench;

use crate::registration::transform::Transform;

/// Count inliers and compute MSAC score using SIMD acceleration.
///
/// For each point pair, computes: dist² = (transform(ref) - target)²
/// Inliers satisfy dist² < threshold². Score = sum(threshold² - dist²) for inliers.
///
/// Reuses the provided `inliers` buffer to avoid allocation. The buffer is cleared
/// before use and filled with inlier indices.
///
/// This function dispatches to the best available SIMD implementation at runtime.
#[inline]
pub(crate) fn count_inliers_simd(
    ref_points: &[DVec2],
    target_points: &[DVec2],
    transform: &Transform,
    threshold_sq: f64,
    inliers: &mut Vec<usize>,
) -> f64 {
    let len = ref_points.len();
    inliers.clear();

    if len == 0 {
        return 0.0;
    }

    #[cfg(target_arch = "x86_64")]
    {
        if len >= 4 && cpu_features::has_avx2() {
            return unsafe {
                sse::count_inliers_avx2(ref_points, target_points, transform, threshold_sq, inliers)
            };
        }
        if len >= 2 && cpu_features::has_sse2() {
            return unsafe {
                sse::count_inliers_sse2(ref_points, target_points, transform, threshold_sq, inliers)
            };
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        if len >= 2 {
            return unsafe {
                neon::count_inliers_neon(
                    ref_points,
                    target_points,
                    transform,
                    threshold_sq,
                    inliers,
                )
            };
        }
    }

    // Scalar fallback
    count_inliers_scalar(ref_points, target_points, transform, threshold_sq, inliers)
}

/// Scalar implementation of inlier counting.
#[cfg(test)]
pub(crate) fn count_inliers_scalar(
    ref_points: &[DVec2],
    target_points: &[DVec2],
    transform: &Transform,
    threshold_sq: f64,
    inliers: &mut Vec<usize>,
) -> f64 {
    count_inliers_scalar_impl(ref_points, target_points, transform, threshold_sq, inliers)
}

#[cfg(not(test))]
fn count_inliers_scalar(
    ref_points: &[DVec2],
    target_points: &[DVec2],
    transform: &Transform,
    threshold_sq: f64,
    inliers: &mut Vec<usize>,
) -> f64 {
    count_inliers_scalar_impl(ref_points, target_points, transform, threshold_sq, inliers)
}

#[inline]
fn count_inliers_scalar_impl(
    ref_points: &[DVec2],
    target_points: &[DVec2],
    transform: &Transform,
    threshold_sq: f64,
    inliers: &mut Vec<usize>,
) -> f64 {
    inliers.clear();
    let mut score = 0.0f64;

    for (i, (r, t)) in ref_points.iter().zip(target_points.iter()).enumerate() {
        let p = transform.apply(*r);
        let dist_sq = (p - *t).length_squared();

        if dist_sq < threshold_sq {
            inliers.push(i);
            score += threshold_sq - dist_sq;
        }
    }

    score
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_transform() -> Transform {
        // Simple translation: shift by (10, 5)
        Transform::translation(DVec2::new(10.0, 5.0))
    }

    fn create_similarity_transform() -> Transform {
        // Rotation of 30 degrees, scale 1.5, translation (10, 20)
        Transform::similarity(DVec2::new(10.0, 20.0), std::f64::consts::PI / 6.0, 1.5)
    }

    #[test]
    fn test_count_inliers_simd_basic() {
        let ref_points = vec![
            DVec2::new(0.0, 0.0),
            DVec2::new(10.0, 10.0),
            DVec2::new(20.0, 20.0),
            DVec2::new(30.0, 30.0),
        ];
        // Target points after translation (10, 5)
        let target_points = vec![
            DVec2::new(10.0, 5.0),
            DVec2::new(20.0, 15.0),
            DVec2::new(30.0, 25.0),
            DVec2::new(40.0, 35.0),
        ];
        let transform = create_test_transform();
        let threshold_sq = 1.0; // threshold = 1.0

        let mut inliers = Vec::new();
        let mut inliers_scalar = Vec::new();
        let score = count_inliers_simd(
            &ref_points,
            &target_points,
            &transform,
            threshold_sq,
            &mut inliers,
        );
        let score_scalar = count_inliers_scalar(
            &ref_points,
            &target_points,
            &transform,
            threshold_sq,
            &mut inliers_scalar,
        );

        assert_eq!(inliers, inliers_scalar);
        assert!((score - score_scalar).abs() < 1e-10);
        assert_eq!(inliers.len(), 4); // All should be inliers (exact match)
    }

    #[test]
    fn test_count_inliers_simd_with_outliers() {
        let ref_points = vec![
            DVec2::new(0.0, 0.0),
            DVec2::new(10.0, 10.0),
            DVec2::new(20.0, 20.0),
            DVec2::new(30.0, 30.0),
        ];
        // Some outliers
        let target_points = vec![
            DVec2::new(10.0, 5.0),    // inlier
            DVec2::new(25.0, 25.0),   // outlier (distance > threshold)
            DVec2::new(30.0, 25.0),   // inlier
            DVec2::new(100.0, 100.0), // outlier
        ];
        let transform = create_test_transform();
        let threshold_sq = 4.0; // threshold = 2.0

        let mut inliers = Vec::new();
        let mut inliers_scalar = Vec::new();
        count_inliers_simd(
            &ref_points,
            &target_points,
            &transform,
            threshold_sq,
            &mut inliers,
        );
        count_inliers_scalar(
            &ref_points,
            &target_points,
            &transform,
            threshold_sq,
            &mut inliers_scalar,
        );

        assert_eq!(inliers, inliers_scalar);
        assert_eq!(inliers.len(), 2); // Only indices 0 and 2
        assert!(inliers.contains(&0));
        assert!(inliers.contains(&2));
    }

    #[test]
    fn test_count_inliers_simd_empty() {
        let ref_points: Vec<DVec2> = Vec::new();
        let target_points: Vec<DVec2> = Vec::new();
        let transform = create_test_transform();

        let mut inliers = Vec::new();
        let score = count_inliers_simd(&ref_points, &target_points, &transform, 1.0, &mut inliers);
        assert!(inliers.is_empty());
        assert!(score == 0.0);
    }

    #[test]
    fn test_count_inliers_simd_matches_scalar_various_sizes() {
        let transform = create_similarity_transform();
        let threshold_sq = 4.0; // threshold = 2.0

        let mut inliers_simd = Vec::new();
        let mut inliers_scalar = Vec::new();
        for size in [1, 2, 3, 4, 5, 7, 8, 9, 15, 16, 17, 32, 64, 100] {
            let ref_points: Vec<DVec2> = (0..size)
                .map(|i| DVec2::new(i as f64 * 10.0, i as f64 * 5.0))
                .collect();

            // Create target points with some transformation applied
            let target_points: Vec<DVec2> = ref_points
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    let t = transform.apply(*p);
                    // Add some noise/outliers
                    if i % 5 == 0 {
                        t + DVec2::new(100.0, 100.0) // outlier
                    } else {
                        t + DVec2::new(0.1, -0.1) // small noise
                    }
                })
                .collect();

            let score_simd = count_inliers_simd(
                &ref_points,
                &target_points,
                &transform,
                threshold_sq,
                &mut inliers_simd,
            );
            let score_scalar = count_inliers_scalar(
                &ref_points,
                &target_points,
                &transform,
                threshold_sq,
                &mut inliers_scalar,
            );

            assert_eq!(
                inliers_simd, inliers_scalar,
                "Size {}: inliers mismatch",
                size
            );
            assert!(
                (score_simd - score_scalar).abs() < 1e-6,
                "Size {}: score mismatch",
                size
            );
        }
    }

    #[test]
    fn test_count_inliers_simd_score_calculation() {
        // Test that score is computed correctly
        let ref_points = vec![DVec2::new(0.0, 0.0), DVec2::new(10.0, 0.0)];
        let target_points = vec![DVec2::new(10.0, 5.0), DVec2::new(20.0, 5.0)]; // Exact match after translation
        let transform = create_test_transform();
        let threshold_sq = 4.0; // threshold = 2.0

        let mut inliers = Vec::new();
        let mut inliers_scalar = Vec::new();
        let score = count_inliers_simd(
            &ref_points,
            &target_points,
            &transform,
            threshold_sq,
            &mut inliers,
        );
        let score_scalar = count_inliers_scalar(
            &ref_points,
            &target_points,
            &transform,
            threshold_sq,
            &mut inliers_scalar,
        );

        assert_eq!(inliers.len(), 2);
        assert!((score - score_scalar).abs() < 1e-10);
        // Score should be 2 * threshold_sq = 2 * 4.0 = 8.0 (exact match, dist_sq = 0)
        assert!((score - 8.0).abs() < 1e-10);
    }
}
