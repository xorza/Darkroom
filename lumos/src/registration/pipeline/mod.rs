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
    triangle::{TriangleMatchConfig, match_stars_triangles_kdtree},
    types::{
        RegistrationConfig, RegistrationError, RegistrationResult, TransformMatrix, TransformType,
    },
};

#[cfg(test)]
mod tests;

#[cfg(feature = "bench")]
pub mod bench;

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

        let matches = match_stars_triangles_kdtree(&ref_stars, &target_stars, &triangle_config);

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
            use_local_optimization: true,
            lo_max_iterations: 10,
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
        clamp_output: false,
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
