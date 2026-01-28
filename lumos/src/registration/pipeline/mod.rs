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
    triangle::{TriangleMatchConfig, match_triangles},
    types::{
        RegistrationConfig, RegistrationError, RegistrationResult, TransformMatrix, TransformType,
    },
};
use crate::star_detection::Star;

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

    /// Register target to reference using pre-detected stars.
    ///
    /// This is the main entry point when stars have been detected.
    /// Stars should be sorted by brightness (flux) in descending order.
    ///
    /// # Arguments
    ///
    /// * `ref_stars` - Detected stars in the reference image (sorted by flux, brightest first)
    /// * `target_stars` - Detected stars in the target image (sorted by flux, brightest first)
    ///
    /// # Returns
    ///
    /// Registration result containing the transformation and quality metrics.
    pub fn register_stars(
        &self,
        ref_stars: &[Star],
        target_stars: &[Star],
    ) -> Result<RegistrationResult, RegistrationError> {
        // Convert to (x, y) tuples and delegate to position-based method
        let ref_positions: Vec<(f64, f64)> =
            ref_stars.iter().map(|s| (s.x as f64, s.y as f64)).collect();
        let target_positions: Vec<(f64, f64)> = target_stars
            .iter()
            .map(|s| (s.x as f64, s.y as f64))
            .collect();

        self.register_positions(&ref_positions, &target_positions)
    }

    /// Register target to reference using star positions as (x, y) tuples.
    ///
    /// This is useful when you have pre-extracted positions or for testing.
    /// Positions should be sorted by brightness (brightest first) for best results.
    ///
    /// # Arguments
    ///
    /// * `ref_positions` - Star positions (x, y) in the reference image
    /// * `target_positions` - Star positions (x, y) in the target image
    ///
    /// # Returns
    ///
    /// Registration result containing the transformation and quality metrics.
    pub fn register_positions(
        &self,
        ref_positions: &[(f64, f64)],
        target_positions: &[(f64, f64)],
    ) -> Result<RegistrationResult, RegistrationError> {
        let start = Instant::now();

        // Validate input
        if ref_positions.len() < self.config.min_stars_for_matching {
            return Err(RegistrationError::InsufficientStars {
                found: ref_positions.len(),
                required: self.config.min_stars_for_matching,
            });
        }
        if target_positions.len() < self.config.min_stars_for_matching {
            return Err(RegistrationError::InsufficientStars {
                found: target_positions.len(),
                required: self.config.min_stars_for_matching,
            });
        }

        // Select stars for matching - either spatially distributed or just brightest N
        let ref_stars = if self.config.use_spatial_distribution {
            select_spatially_distributed(
                ref_positions,
                self.config.max_stars_for_matching,
                self.config.spatial_grid_size,
            )
        } else {
            ref_positions
                .iter()
                .take(self.config.max_stars_for_matching)
                .copied()
                .collect()
        };
        let target_stars = if self.config.use_spatial_distribution {
            select_spatially_distributed(
                target_positions,
                self.config.max_stars_for_matching,
                self.config.spatial_grid_size,
            )
        } else {
            target_positions
                .iter()
                .take(self.config.max_stars_for_matching)
                .copied()
                .collect()
        };

        // Step 1: Triangle matching to find star correspondences
        let triangle_config = TriangleMatchConfig {
            max_stars: self.config.max_stars_for_matching,
            ratio_tolerance: self.config.triangle_tolerance,
            min_votes: 2,
            hash_bins: 100,
            check_orientation: true,
            two_step_matching: false, // Standard single-pass matching
        };

        let matches = match_triangles(&ref_stars, &target_stars, &triangle_config);

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
            .ok_or(RegistrationError::RansacFailed {
                reason: crate::registration::types::RansacFailureReason::NoInliersFound,
                iterations: self.config.ransac_iterations,
                best_inlier_count: 0,
            })?;

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
        let mut result = self.register_positions(ref_stars, &adjusted_target_stars)?;

        // If we used phase correlation, compose the transforms
        if let Some(pr) = phase_result {
            let (dx, dy) = pr.translation;
            let phase_transform = TransformMatrix::translation(dx, dy);
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

/// Convenience function to register two star position lists.
///
/// This is a shorthand for creating a Registrator with default settings.
/// For more control, use `Registrator::new()` with a custom config.
///
/// # Arguments
///
/// * `ref_positions` - Reference star positions (x, y)
/// * `target_positions` - Target star positions (x, y)
/// * `transform_type` - Type of geometric transformation to estimate
///
/// # Example
/// ```rust,ignore
/// use lumos::{register_star_positions, TransformType};
///
/// let ref_positions = vec![
///     (100.0, 200.0), (300.0, 150.0), (250.0, 400.0),
///     (500.0, 300.0), (150.0, 350.0), (450.0, 100.0),
/// ];
/// let target_positions = vec![
///     (110.0, 205.0), (310.0, 155.0), (260.0, 405.0),
///     (510.0, 305.0), (160.0, 355.0), (460.0, 105.0),
/// ];
///
/// let result = register_star_positions(&ref_positions, &target_positions, TransformType::Affine)?;
/// println!("RMS error: {:.3} pixels", result.rms_error);
/// println!("Matched {} stars", result.num_inliers);
/// ```
pub fn register_star_positions(
    ref_positions: &[(f64, f64)],
    target_positions: &[(f64, f64)],
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

    Registrator::new(config).register_positions(ref_positions, target_positions)
}

/// Warp target image to align with reference.
///
/// # Arguments
///
/// * `target_image` - Target image pixel data
/// * `width` - Image width
/// * `height` - Image height
/// * `transform` - Transformation from reference to target coordinates (as returned by `register_stars`)
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

    // The transform maps reference→target coordinates.
    // warp_image expects input→output transform (target→reference here),
    // then inverts it internally to get output→input for sampling.
    // So we pass the inverse (target→reference), which gets inverted back
    // to reference→target, giving us the correct sampling direction.
    let inverse = transform.inverse();

    warp_image(
        target_image,
        width,
        height,
        width,
        height,
        &inverse,
        &config,
    )
}

/// Warp an AstroImage to align with reference.
///
/// Handles both single-channel (grayscale) and multi-channel (RGB) images.
/// For multi-channel images, each channel is warped independently in parallel.
///
/// # Arguments
///
/// * `target` - Target image to warp
/// * `transform` - Transformation from reference to target coordinates (as returned by `register_stars`)
/// * `method` - Interpolation method
///
/// # Returns
///
/// Warped image aligned to reference frame.
pub fn warp_to_reference_image(
    target: &crate::AstroImage,
    transform: &TransformMatrix,
    method: InterpolationMethod,
) -> crate::AstroImage {
    use rayon::prelude::*;

    let width = target.width();
    let height = target.height();
    let channels = target.channels();
    let pixels = target.pixels();

    if channels == 1 {
        // Single channel: direct warping
        let warped = warp_to_reference(pixels, width, height, transform, method);
        let mut result = crate::AstroImage::from_pixels(width, height, 1, warped);
        result.metadata = target.metadata.clone();
        result
    } else {
        // Multi-channel: extract, warp each channel in parallel, interleave
        let warped_channels: Vec<Vec<f32>> = (0..channels)
            .into_par_iter()
            .map(|c| {
                let channel: Vec<f32> = pixels.iter().skip(c).step_by(channels).copied().collect();
                warp_to_reference(&channel, width, height, transform, method)
            })
            .collect();

        // Interleave channels back together
        let mut warped_pixels = vec![0.0f32; width * height * channels];
        for (c, channel_data) in warped_channels.iter().enumerate() {
            for (i, &val) in channel_data.iter().enumerate() {
                warped_pixels[i * channels + c] = val;
            }
        }

        let mut result = crate::AstroImage::from_pixels(width, height, channels, warped_pixels);
        result.metadata = target.metadata.clone();
        result
    }
}

/// Quick registration using default settings with position tuples.
///
/// Suitable for well-aligned images with good star coverage.
/// Returns only the transformation matrix. For full registration result
/// with quality metrics, use `quick_register_stars`.
///
/// # Arguments
///
/// * `ref_stars` - Reference star positions (x, y)
/// * `target_stars` - Target star positions (x, y)
///
/// # Returns
///
/// The transformation matrix that maps reference to target coordinates.
///
/// # Example
/// ```rust,ignore
/// use lumos::{quick_register, warp_to_reference, InterpolationMethod};
///
/// // Star positions detected from both images
/// let ref_positions = vec![(100.0, 200.0), (300.0, 150.0), /* ... */];
/// let target_positions = vec![(102.0, 198.0), (302.0, 148.0), /* ... */];
///
/// // Get the transformation matrix
/// let transform = quick_register(&ref_positions, &target_positions)?;
///
/// // Warp target image to align with reference
/// let aligned = warp_to_reference(&target_pixels, width, height, &transform, InterpolationMethod::Lanczos3);
/// ```
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

    let result = Registrator::new(config).register_positions(ref_stars, target_stars)?;
    Ok(result.transform)
}

/// Quick registration using detected stars with sensible defaults.
///
/// Convenience function for common registration scenarios. Uses:
/// - Affine transformation (6 DOF: translation, rotation, scale, shear)
/// - Standard RANSAC with 1000 iterations
/// - 2-pixel inlier threshold
/// - Up to 100 stars for matching
///
/// For more control over registration parameters, use `Registrator::new()` with
/// a custom `RegistrationConfig`.
///
/// # Arguments
///
/// * `ref_stars` - Detected stars in the reference image (sorted by flux, brightest first)
/// * `target_stars` - Detected stars in the target image (sorted by flux, brightest first)
///
/// # Returns
///
/// Full registration result including transformation, inlier matches, and quality metrics.
///
/// # Example
///
/// ```ignore
/// use lumos::{find_stars, quick_register_stars, StarDetectionConfig};
///
/// let ref_stars = find_stars(&ref_image, &StarDetectionConfig::default())?;
/// let target_stars = find_stars(&target_image, &StarDetectionConfig::default())?;
///
/// let result = quick_register_stars(&ref_stars.stars, &target_stars.stars)?;
/// println!("RMS error: {:.3} pixels", result.rms_error);
/// println!("Matched {} stars", result.num_inliers);
/// ```
pub fn quick_register_stars(
    ref_stars: &[Star],
    target_stars: &[Star],
) -> Result<RegistrationResult, RegistrationError> {
    let config = RegistrationConfig::builder()
        .full_affine() // 6 DOF for handling rotation, scale, and shear
        .ransac_iterations(1000)
        .ransac_threshold(2.0)
        .max_stars(100)
        .min_matched_stars(4)
        .max_residual(5.0)
        .build();

    Registrator::new(config).register_stars(ref_stars, target_stars)
}

/// Multi-scale registration configuration.
#[derive(Debug, Clone)]
pub struct MultiScaleConfig {
    /// Number of pyramid levels (1 = no pyramid, just full resolution)
    pub levels: usize,
    /// Scale factor between levels (typically 2.0 for half-resolution each level)
    pub scale_factor: f64,
    /// Minimum image dimension at coarsest level
    pub min_dimension: usize,
    /// Whether to use phase correlation at coarse levels
    pub use_phase_correlation: bool,
}

impl Default for MultiScaleConfig {
    fn default() -> Self {
        Self {
            levels: 3,
            scale_factor: 2.0,
            min_dimension: 128,
            use_phase_correlation: true,
        }
    }
}

/// Multi-scale image registration.
///
/// Uses a pyramid approach to speed up registration of large images:
/// 1. Build an image pyramid by successive downsampling
/// 2. Register at the coarsest level first
/// 3. Use the coarse result as initial estimate for finer levels
/// 4. Refine progressively until full resolution
///
/// This is particularly effective for:
/// - Large images where full-resolution star matching is slow
/// - Images with significant translation that could confuse matching
/// - Reducing false matches by constraining search space at fine levels
#[derive(Debug)]
pub struct MultiScaleRegistrator {
    config: RegistrationConfig,
    multiscale_config: MultiScaleConfig,
}

impl MultiScaleRegistrator {
    /// Create a new multi-scale registrator.
    pub fn new(config: RegistrationConfig, multiscale_config: MultiScaleConfig) -> Self {
        config.validate();
        Self {
            config,
            multiscale_config,
        }
    }

    /// Register using pre-detected stars at multiple scales.
    ///
    /// Stars should be detected at full resolution. They will be scaled
    /// appropriately for each pyramid level.
    ///
    /// # Arguments
    /// * `ref_stars` - Reference star positions at full resolution
    /// * `target_stars` - Target star positions at full resolution
    /// * `image_width` - Full resolution image width
    /// * `image_height` - Full resolution image height
    pub fn register_stars(
        &self,
        ref_stars: &[(f64, f64)],
        target_stars: &[(f64, f64)],
        image_width: usize,
        image_height: usize,
    ) -> Result<RegistrationResult, RegistrationError> {
        let start = Instant::now();

        // Calculate actual number of levels based on image size
        let min_dim = image_width.min(image_height);
        let max_levels = ((min_dim as f64 / self.multiscale_config.min_dimension as f64).log2()
            / self.multiscale_config.scale_factor.log2())
        .floor() as usize
            + 1;
        let num_levels = self.multiscale_config.levels.min(max_levels).max(1);

        if num_levels == 1 {
            // Single level - just do normal registration
            return Registrator::new(self.config.clone())
                .register_positions(ref_stars, target_stars);
        }

        // Start from coarsest level and work down
        let mut current_transform = TransformMatrix::identity();
        let mut last_result: Option<RegistrationResult> = None;

        for level in (0..num_levels).rev() {
            let scale = self.multiscale_config.scale_factor.powi(level as i32);

            // Scale star positions to this level
            let scaled_ref: Vec<(f64, f64)> = ref_stars
                .iter()
                .map(|(x, y)| (x / scale, y / scale))
                .collect();

            // Apply current transform estimate to target stars, then scale
            let adjusted_target: Vec<(f64, f64)> = target_stars
                .iter()
                .map(|(x, y)| {
                    // Apply inverse of current estimate to pre-align
                    let inv = current_transform.inverse();
                    let (ax, ay) = inv.apply(*x, *y);
                    (ax / scale, ay / scale)
                })
                .collect();

            // Use relaxed config for coarse levels, stricter for fine
            let level_config = if level > 0 {
                RegistrationConfig {
                    ransac_threshold: self.config.ransac_threshold * scale.sqrt(),
                    triangle_tolerance: self.config.triangle_tolerance * 1.5,
                    ransac_iterations: self.config.ransac_iterations / 2,
                    max_residual_pixels: self.config.max_residual_pixels * scale,
                    ..self.config.clone()
                }
            } else {
                self.config.clone()
            };

            let registrator = Registrator::new(level_config);
            match registrator.register_positions(&scaled_ref, &adjusted_target) {
                Ok(result) => {
                    // Scale the transform back to full resolution
                    let level_transform = scale_transform(&result.transform, scale);

                    // Compose with previous estimate
                    current_transform = level_transform.compose(&current_transform);
                    last_result = Some(result);
                }
                Err(e) => {
                    // If coarse level fails, try next finer level
                    if level > 0 {
                        continue;
                    } else {
                        return Err(e);
                    }
                }
            }
        }

        // Return the final result with updated transform
        match last_result {
            Some(mut result) => {
                result.transform = current_transform;
                result.elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
                Ok(result)
            }
            None => Err(RegistrationError::NoMatchingPatterns),
        }
    }

    /// Register using images and stars at multiple scales.
    ///
    /// Uses phase correlation at coarse levels for initial alignment,
    /// then refines with star matching at finer levels.
    pub fn register_with_images(
        &self,
        ref_image: &[f32],
        target_image: &[f32],
        width: usize,
        height: usize,
        ref_stars: &[(f64, f64)],
        target_stars: &[(f64, f64)],
    ) -> Result<RegistrationResult, RegistrationError> {
        let start = Instant::now();

        if ref_image.len() != width * height || target_image.len() != width * height {
            return Err(RegistrationError::DimensionMismatch);
        }

        // Calculate actual number of levels
        let min_dim = width.min(height);
        let max_levels = ((min_dim as f64 / self.multiscale_config.min_dimension as f64).log2()
            / self.multiscale_config.scale_factor.log2())
        .floor() as usize
            + 1;
        let num_levels = self.multiscale_config.levels.min(max_levels).max(1);

        let mut current_transform = TransformMatrix::identity();

        // Build image pyramid
        let ref_pyramid = build_pyramid(
            ref_image,
            width,
            height,
            num_levels,
            self.multiscale_config.scale_factor,
        );
        let target_pyramid = build_pyramid(
            target_image,
            width,
            height,
            num_levels,
            self.multiscale_config.scale_factor,
        );

        // Process from coarse to fine
        for level in (0..num_levels).rev() {
            let scale = self.multiscale_config.scale_factor.powi(level as i32);
            let (level_ref, level_width, level_height) = &ref_pyramid[level];
            let (level_target, _, _) = &target_pyramid[level];

            // At coarse levels, use phase correlation if enabled
            if level > 0 && self.multiscale_config.use_phase_correlation {
                let phase_config = PhaseCorrelationConfig::default();
                let correlator = PhaseCorrelator::new(*level_width, *level_height, phase_config);

                if let Some(pr) =
                    correlator.correlate(level_ref, level_target, *level_width, *level_height)
                {
                    let (dx, dy) = pr.translation;
                    // Scale translation to full resolution
                    let phase_transform = TransformMatrix::translation(dx * scale, dy * scale);
                    current_transform = phase_transform.compose(&current_transform);
                }
            }

            // Scale star positions and do star matching
            let scaled_ref: Vec<(f64, f64)> = ref_stars
                .iter()
                .map(|(x, y)| (x / scale, y / scale))
                .collect();

            let adjusted_target: Vec<(f64, f64)> = target_stars
                .iter()
                .map(|(x, y)| {
                    let inv = current_transform.inverse();
                    let (ax, ay) = inv.apply(*x, *y);
                    (ax / scale, ay / scale)
                })
                .collect();

            let level_config = if level > 0 {
                RegistrationConfig {
                    ransac_threshold: self.config.ransac_threshold * scale.sqrt(),
                    triangle_tolerance: self.config.triangle_tolerance * 1.5,
                    ransac_iterations: self.config.ransac_iterations / 2,
                    max_residual_pixels: self.config.max_residual_pixels * scale,
                    ..self.config.clone()
                }
            } else {
                self.config.clone()
            };

            let registrator = Registrator::new(level_config);
            if let Ok(result) = registrator.register_positions(&scaled_ref, &adjusted_target) {
                let level_transform = scale_transform(&result.transform, scale);
                current_transform = level_transform.compose(&current_transform);
            }
        }

        // Final registration at full resolution with refined estimate
        let adjusted_target: Vec<(f64, f64)> = target_stars
            .iter()
            .map(|(x, y)| {
                let inv = current_transform.inverse();
                inv.apply(*x, *y)
            })
            .collect();

        let registrator = Registrator::new(self.config.clone());
        let mut result = registrator.register_positions(ref_stars, &adjusted_target)?;

        // Compose final transform
        result.transform = result.transform.compose(&current_transform);
        result.elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;

        Ok(result)
    }
}

/// Scale a transformation from one resolution to another.
fn scale_transform(transform: &TransformMatrix, scale: f64) -> TransformMatrix {
    // For a transform T that works at scale s, the equivalent at scale 1 is:
    // Scale the translation by s, keep rotation/scale the same
    let mut data = transform.data;
    data[2] *= scale; // tx
    data[5] *= scale; // ty
    TransformMatrix::matrix(data, transform.transform_type)
}

/// Build an image pyramid by successive downsampling.
fn build_pyramid(
    image: &[f32],
    width: usize,
    height: usize,
    levels: usize,
    scale_factor: f64,
) -> Vec<(Vec<f32>, usize, usize)> {
    let mut pyramid = Vec::with_capacity(levels);

    // Level 0 is full resolution
    pyramid.push((image.to_vec(), width, height));

    let mut current_width = width;
    let mut current_height = height;
    let mut current_image = image.to_vec();

    for _ in 1..levels {
        let new_width = (current_width as f64 / scale_factor).round() as usize;
        let new_height = (current_height as f64 / scale_factor).round() as usize;

        if new_width < 2 || new_height < 2 {
            break;
        }

        let downsampled = downsample_image(
            &current_image,
            current_width,
            current_height,
            new_width,
            new_height,
        );

        current_width = new_width;
        current_height = new_height;
        current_image = downsampled.clone();

        pyramid.push((downsampled, new_width, new_height));
    }

    pyramid
}

/// Downsample an image using box filtering.
fn downsample_image(
    image: &[f32],
    src_width: usize,
    src_height: usize,
    dst_width: usize,
    dst_height: usize,
) -> Vec<f32> {
    let mut result = vec![0.0f32; dst_width * dst_height];

    let scale_x = src_width as f64 / dst_width as f64;
    let scale_y = src_height as f64 / dst_height as f64;

    for dy in 0..dst_height {
        for dx in 0..dst_width {
            // Compute source region
            let sx0 = (dx as f64 * scale_x) as usize;
            let sy0 = (dy as f64 * scale_y) as usize;
            let sx1 = ((dx + 1) as f64 * scale_x).ceil() as usize;
            let sy1 = ((dy + 1) as f64 * scale_y).ceil() as usize;

            let sx1 = sx1.min(src_width);
            let sy1 = sy1.min(src_height);

            // Box filter (average)
            let mut sum = 0.0;
            let mut count = 0;

            for sy in sy0..sy1 {
                for sx in sx0..sx1 {
                    sum += image[sy * src_width + sx];
                    count += 1;
                }
            }

            result[dy * dst_width + dx] = if count > 0 { sum / count as f32 } else { 0.0 };
        }
    }

    result
}

/// Select stars with good spatial distribution across the image.
///
/// Instead of just taking the brightest N stars (which may cluster in one region),
/// this function divides the image into a grid and selects the brightest stars
/// from each cell in round-robin fashion, ensuring coverage across the entire field.
///
/// # Arguments
///
/// * `stars` - Star positions (x, y), assumed sorted by brightness (brightest first)
/// * `max_stars` - Maximum number of stars to select
/// * `grid_size` - Number of grid cells in each dimension (grid_size × grid_size)
///
/// # Returns
///
/// Selected star positions with good spatial coverage.
fn select_spatially_distributed(
    stars: &[(f64, f64)],
    max_stars: usize,
    grid_size: usize,
) -> Vec<(f64, f64)> {
    if stars.is_empty() || max_stars == 0 {
        return Vec::new();
    }

    // Find bounding box of all stars
    let (min_x, max_x, min_y, max_y) = stars.iter().fold(
        (f64::MAX, f64::MIN, f64::MAX, f64::MIN),
        |(min_x, max_x, min_y, max_y), &(x, y)| {
            (min_x.min(x), max_x.max(x), min_y.min(y), max_y.max(y))
        },
    );

    // Add small margin to avoid edge issues
    let margin = 1.0;
    let width = (max_x - min_x + 2.0 * margin).max(1.0);
    let height = (max_y - min_y + 2.0 * margin).max(1.0);
    let origin_x = min_x - margin;
    let origin_y = min_y - margin;

    let cell_width = width / grid_size as f64;
    let cell_height = height / grid_size as f64;

    // Group stars by grid cell
    let num_cells = grid_size * grid_size;
    let mut cells: Vec<Vec<(f64, f64)>> = vec![Vec::new(); num_cells];

    for &(x, y) in stars {
        let cx = ((x - origin_x) / cell_width) as usize;
        let cy = ((y - origin_y) / cell_height) as usize;
        let cx = cx.min(grid_size - 1);
        let cy = cy.min(grid_size - 1);
        cells[cy * grid_size + cx].push((x, y));
    }

    // Round-robin selection: take one star from each non-empty cell in turn
    // Stars in each cell are already in brightness order (from input sorting)
    let mut selected = Vec::with_capacity(max_stars);
    let mut round = 0;

    while selected.len() < max_stars {
        let mut added_any = false;
        for cell in &cells {
            if round < cell.len() && selected.len() < max_stars {
                selected.push(cell[round]);
                added_any = true;
            }
        }
        if !added_any {
            break; // No more stars available
        }
        round += 1;
    }

    selected
}
