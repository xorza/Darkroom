//! Full image registration pipeline.
//!
//! This module provides the high-level API for registering astronomical images,
//! combining all the submodules into a complete workflow.
//!
//! # Pipeline Stages
//!
//! 1. **Star Detection** - Extract star positions from both images
//! 2. **Coarse Alignment** (optional) - Use phase correlation for initial estimate
//! 3. **Triangle Matching** - Find star correspondences using geometric hashing
//! 4. **RANSAC** - Robustly estimate transformation with outlier rejection
//! 5. **Refinement** - Optimize transformation using all inliers
//! 6. **Warping** - Apply transformation to align target to reference

use std::time::Instant;

use crate::registration::{
    interpolation::{InterpolationMethod, WarpConfig, warp_image},
    phase_correlation::{PhaseCorrelationConfig, PhaseCorrelator},
    ransac::{RansacConfig, RansacEstimator},
    triangle::{TriangleMatchConfig, match_stars_triangles},
    types::{
        RegistrationConfig, RegistrationError, RegistrationResult, TransformMatrix, TransformType,
    },
};

/// Image registrator that aligns target images to a reference.
#[derive(Debug)]
pub struct Registrator {
    config: RegistrationConfig,
}

impl Default for Registrator {
    fn default() -> Self {
        Self::new(RegistrationConfig::default())
    }
}

impl Registrator {
    /// Create a new registrator with the given configuration.
    pub fn new(config: RegistrationConfig) -> Self {
        config.validate();
        Self { config }
    }

    /// Create a registrator with default configuration.
    pub fn with_defaults() -> Self {
        Self::default()
    }

    /// Register target to reference using pre-detected star positions.
    ///
    /// This is the main entry point when star positions are already known.
    ///
    /// # Arguments
    ///
    /// * `ref_stars` - Star positions (x, y) in the reference image
    /// * `target_stars` - Star positions (x, y) in the target image
    ///
    /// # Returns
    ///
    /// Registration result containing the transformation and quality metrics.
    pub fn register_stars(
        &self,
        ref_stars: &[(f64, f64)],
        target_stars: &[(f64, f64)],
    ) -> Result<RegistrationResult, RegistrationError> {
        let start = Instant::now();

        // Validate input
        if ref_stars.len() < self.config.min_stars_for_matching {
            return Err(RegistrationError::InsufficientStars {
                found: ref_stars.len(),
                required: self.config.min_stars_for_matching,
            });
        }
        if target_stars.len() < self.config.min_stars_for_matching {
            return Err(RegistrationError::InsufficientStars {
                found: target_stars.len(),
                required: self.config.min_stars_for_matching,
            });
        }

        // Limit stars to brightest N (assuming input is sorted by brightness)
        let ref_stars: Vec<_> = ref_stars
            .iter()
            .take(self.config.max_stars_for_matching)
            .copied()
            .collect();
        let target_stars: Vec<_> = target_stars
            .iter()
            .take(self.config.max_stars_for_matching)
            .copied()
            .collect();

        // Step 1: Triangle matching to find star correspondences
        let triangle_config = TriangleMatchConfig {
            max_stars: self.config.max_stars_for_matching,
            ratio_tolerance: self.config.triangle_tolerance,
            min_votes: 2,
            hash_bins: 100,
            check_orientation: true,
        };

        let matches = match_stars_triangles(&ref_stars, &target_stars, &triangle_config);

        if matches.len() < self.config.min_matched_stars {
            return Err(RegistrationError::NoMatchingPatterns);
        }

        // Extract matched point pairs
        let ref_matched: Vec<_> = matches.iter().map(|m| ref_stars[m.ref_idx]).collect();
        let target_matched: Vec<_> = matches.iter().map(|m| target_stars[m.target_idx]).collect();

        // Step 2: RANSAC to robustly estimate transformation
        let ransac_config = RansacConfig {
            max_iterations: self.config.ransac_iterations,
            inlier_threshold: self.config.ransac_threshold,
            confidence: self.config.ransac_confidence,
            min_inlier_ratio: 0.3,
            seed: None,
        };

        let ransac = RansacEstimator::new(ransac_config);
        let ransac_result = ransac
            .estimate(&ref_matched, &target_matched, self.config.transform_type)
            .ok_or(RegistrationError::RansacFailed)?;

        // Get inlier matches
        let inlier_matches: Vec<_> = ransac_result
            .inliers
            .iter()
            .map(|&i| (matches[i].ref_idx, matches[i].target_idx))
            .collect();

        // Compute residuals for inliers
        let residuals: Vec<f64> = ransac_result
            .inliers
            .iter()
            .map(|&i| {
                let (rx, ry): (f64, f64) = ref_matched[i];
                let (tx, ty): (f64, f64) = target_matched[i];
                let (px, py) = ransac_result.transform.apply(rx, ry);
                ((px - tx).powi(2) + (py - ty).powi(2)).sqrt()
            })
            .collect();

        let result = RegistrationResult::new(ransac_result.transform, inlier_matches, residuals)
            .with_elapsed(start.elapsed().as_secs_f64() * 1000.0);

        // Check accuracy
        if result.rms_error > self.config.max_residual_pixels {
            return Err(RegistrationError::AccuracyTooLow {
                rms_error: result.rms_error,
                max_allowed: self.config.max_residual_pixels,
            });
        }

        Ok(result)
    }

    /// Register target to reference using pixel data for phase correlation.
    ///
    /// Uses phase correlation for coarse alignment before star matching.
    ///
    /// # Arguments
    ///
    /// * `ref_image` - Reference image pixel data
    /// * `target_image` - Target image pixel data
    /// * `width` - Image width
    /// * `height` - Image height
    /// * `ref_stars` - Star positions in reference image
    /// * `target_stars` - Star positions in target image
    pub fn register_with_phase_correlation(
        &self,
        ref_image: &[f32],
        target_image: &[f32],
        width: usize,
        height: usize,
        ref_stars: &[(f64, f64)],
        target_stars: &[(f64, f64)],
    ) -> Result<RegistrationResult, RegistrationError> {
        let start = Instant::now();

        // Validate dimensions
        if ref_image.len() != width * height || target_image.len() != width * height {
            return Err(RegistrationError::DimensionMismatch);
        }

        // Phase correlation for coarse alignment
        let phase_config = PhaseCorrelationConfig::default();
        let correlator = PhaseCorrelator::new(width, height, phase_config);

        let phase_result = correlator.correlate(ref_image, target_image, width, height);

        // Apply coarse translation to target stars if phase correlation succeeded
        let adjusted_target_stars: Vec<_> = if let Some(ref pr) = phase_result {
            let (dx, dy) = pr.translation;
            target_stars.iter().map(|(x, y)| (x - dx, y - dy)).collect()
        } else {
            target_stars.to_vec()
        };

        // Now do star-based registration
        let mut result = self.register_stars(ref_stars, &adjusted_target_stars)?;

        // If we used phase correlation, compose the transforms
        if let Some(pr) = phase_result {
            let (dx, dy) = pr.translation;
            let phase_transform = TransformMatrix::from_translation(dx, dy);
            result.transform = result.transform.compose(&phase_transform);
        }

        result.elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
        Ok(result)
    }

    /// Get the current configuration.
    pub fn config(&self) -> &RegistrationConfig {
        &self.config
    }
}

/// Convenience function to register two star lists.
pub fn register_stars(
    ref_stars: &[(f64, f64)],
    target_stars: &[(f64, f64)],
    transform_type: TransformType,
) -> Result<RegistrationResult, RegistrationError> {
    let config = RegistrationConfig::builder()
        .min_stars(6)
        .min_matched_stars(4)
        .max_residual(2.0)
        .build();

    let config = RegistrationConfig {
        transform_type,
        ..config
    };

    Registrator::new(config).register_stars(ref_stars, target_stars)
}

/// Warp target image to align with reference.
///
/// # Arguments
///
/// * `target_image` - Target image pixel data
/// * `width` - Image width
/// * `height` - Image height
/// * `transform` - Transformation from reference to target coordinates
/// * `method` - Interpolation method
///
/// # Returns
///
/// Warped image aligned to reference frame.
pub fn warp_to_reference(
    target_image: &[f32],
    width: usize,
    height: usize,
    transform: &TransformMatrix,
    method: InterpolationMethod,
) -> Vec<f32> {
    let config = WarpConfig {
        method,
        border_value: 0.0,
        normalize_kernel: true,
    };

    warp_image(
        target_image,
        width,
        height,
        width,
        height,
        transform,
        &config,
    )
}

/// Quick registration using default settings.
///
/// Suitable for well-aligned images with good star coverage.
pub fn quick_register(
    ref_stars: &[(f64, f64)],
    target_stars: &[(f64, f64)],
) -> Result<TransformMatrix, RegistrationError> {
    let config = RegistrationConfig::builder()
        .with_scale()
        .ransac_iterations(500)
        .max_stars(100)
        .min_matched_stars(4)
        .build();

    let result = Registrator::new(config).register_stars(ref_stars, target_stars)?;
    Ok(result.transform)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    fn generate_star_grid(
        rows: usize,
        cols: usize,
        spacing: f64,
        offset: (f64, f64),
    ) -> Vec<(f64, f64)> {
        let mut stars = Vec::new();
        for r in 0..rows {
            for c in 0..cols {
                let x = offset.0 + c as f64 * spacing;
                let y = offset.1 + r as f64 * spacing;
                stars.push((x, y));
            }
        }
        stars
    }

    fn transform_stars(stars: &[(f64, f64)], transform: &TransformMatrix) -> Vec<(f64, f64)> {
        stars.iter().map(|&(x, y)| transform.apply(x, y)).collect()
    }

    #[test]
    fn test_registration_identity() {
        let ref_stars = generate_star_grid(5, 5, 100.0, (100.0, 100.0));
        let target_stars = ref_stars.clone();

        let result = register_stars(&ref_stars, &target_stars, TransformType::Translation).unwrap();

        // Should find near-identity transform
        let (tx, ty) = result.transform.translation_components();
        assert!(tx.abs() < 1.0, "Expected near-zero translation, got {}", tx);
        assert!(ty.abs() < 1.0, "Expected near-zero translation, got {}", ty);
        assert!(result.rms_error < 0.5, "Expected low RMS error");
    }

    #[test]
    fn test_registration_translation() {
        let ref_stars = generate_star_grid(5, 5, 100.0, (100.0, 100.0));
        let translation = TransformMatrix::from_translation(50.0, -30.0);
        let target_stars = transform_stars(&ref_stars, &translation);

        let result = register_stars(&ref_stars, &target_stars, TransformType::Translation).unwrap();

        let (tx, ty) = result.transform.translation_components();
        assert!((tx - 50.0).abs() < 1.0, "Expected tx=50, got {}", tx);
        assert!((ty - (-30.0)).abs() < 1.0, "Expected ty=-30, got {}", ty);
    }

    #[test]
    fn test_registration_rotation() {
        let ref_stars = generate_star_grid(5, 5, 100.0, (200.0, 200.0));
        let rotation = TransformMatrix::euclidean(10.0, -5.0, 0.1); // ~5.7 degrees
        let target_stars = transform_stars(&ref_stars, &rotation);

        let result = register_stars(&ref_stars, &target_stars, TransformType::Euclidean).unwrap();

        let angle = result.transform.rotation_angle();
        assert!(
            (angle - 0.1).abs() < 0.01,
            "Expected angle=0.1, got {}",
            angle
        );
    }

    #[test]
    fn test_registration_similarity() {
        let ref_stars = generate_star_grid(5, 5, 100.0, (200.0, 200.0));
        let similarity = TransformMatrix::similarity(20.0, 15.0, 0.05, 1.02);
        let target_stars = transform_stars(&ref_stars, &similarity);

        let result = register_stars(&ref_stars, &target_stars, TransformType::Similarity).unwrap();

        let scale = result.transform.scale_factor();
        assert!(
            (scale - 1.02).abs() < 0.01,
            "Expected scale=1.02, got {}",
            scale
        );
    }

    #[test]
    fn test_registration_with_outliers() {
        let ref_stars = generate_star_grid(6, 6, 80.0, (100.0, 100.0));
        let translation = TransformMatrix::from_translation(25.0, 40.0);
        let mut target_stars = transform_stars(&ref_stars, &translation);

        // Add outliers (wrong matches)
        target_stars[0] = (500.0, 500.0);
        target_stars[5] = (50.0, 800.0);
        target_stars[10] = (900.0, 100.0);

        let result = register_stars(&ref_stars, &target_stars, TransformType::Translation).unwrap();

        let (tx, ty) = result.transform.translation_components();
        // RANSAC should still find correct translation despite outliers
        assert!((tx - 25.0).abs() < 2.0, "Expected tx=25, got {}", tx);
        assert!((ty - 40.0).abs() < 2.0, "Expected ty=40, got {}", ty);
    }

    #[test]
    fn test_registration_insufficient_stars() {
        let ref_stars = vec![(100.0, 100.0), (200.0, 200.0)];
        let target_stars = ref_stars.clone();

        let result = register_stars(&ref_stars, &target_stars, TransformType::Translation);
        assert!(matches!(
            result,
            Err(RegistrationError::InsufficientStars { .. })
        ));
    }

    #[test]
    fn test_registrator_config() {
        let config = RegistrationConfig::builder()
            .with_rotation()
            .ransac_iterations(2000)
            .ransac_threshold(1.5)
            .build();

        let registrator = Registrator::new(config);
        assert_eq!(registrator.config().ransac_iterations, 2000);
        assert!((registrator.config().ransac_threshold - 1.5).abs() < 1e-10);
    }

    #[test]
    fn test_warp_to_reference() {
        // Create a simple test image
        let width = 64;
        let height = 64;
        let mut image = vec![0.0f32; width * height];
        image[32 * width + 32] = 1.0; // Bright pixel at center

        let transform = TransformMatrix::from_translation(5.0, 3.0);
        let warped = warp_to_reference(
            &image,
            width,
            height,
            &transform,
            InterpolationMethod::Bilinear,
        );

        assert_eq!(warped.len(), width * height);
        // The bright pixel should have moved
        assert!(warped[32 * width + 32] < 0.5);
    }

    #[test]
    fn test_quick_register() {
        let ref_stars = generate_star_grid(4, 4, 150.0, (100.0, 100.0));
        let translation = TransformMatrix::from_translation(10.0, -15.0);
        let target_stars = transform_stars(&ref_stars, &translation);

        let transform = quick_register(&ref_stars, &target_stars).unwrap();
        let (tx, ty) = transform.translation_components();

        assert!((tx - 10.0).abs() < 1.0);
        assert!((ty - (-15.0)).abs() < 1.0);
    }

    #[test]
    fn test_registration_result_quality() {
        let ref_stars = generate_star_grid(6, 6, 100.0, (50.0, 50.0));
        let target_stars = ref_stars.clone();

        let result = register_stars(&ref_stars, &target_stars, TransformType::Translation).unwrap();

        // Perfect match should have very low error and high quality
        assert!(result.rms_error < 0.1);
        assert!(result.num_inliers >= 20);
    }

    #[test]
    fn test_registration_large_rotation() {
        let ref_stars = generate_star_grid(5, 5, 100.0, (250.0, 250.0));
        // 30 degree rotation around image center
        let rotation = TransformMatrix::from_rotation_around(PI / 6.0, 300.0, 300.0);
        let target_stars = transform_stars(&ref_stars, &rotation);

        let result = register_stars(&ref_stars, &target_stars, TransformType::Euclidean).unwrap();

        let angle = result.transform.rotation_angle();
        assert!(
            (angle - PI / 6.0).abs() < 0.05,
            "Expected 30deg rotation, got {} rad",
            angle
        );
    }
}
