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

mod result;

pub use result::{RansacFailureReason, RegistrationError, RegistrationResult};

use glam::DVec2;
use std::time::Instant;

use crate::ImageDimensions;
use crate::common::Buffer2;
use crate::registration::{
    config::{InterpolationMethod, MultiScaleConfig, RegistrationConfig, WarpConfig},
    distortion::{SipConfig, SipPolynomial},
    interpolation::warp_image,
    phase_correlation::PhaseCorrelator,
    ransac::{RansacEstimator, estimate_transform},
    spatial::KdTree,
    transform::Transform,
    transform::TransformType,
    triangle::{StarMatch, match_triangles},
};
use crate::star_detection::Star;

#[cfg(test)]
mod tests;

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
        // Convert to DVec2 and delegate to position-based method
        let ref_positions: Vec<DVec2> = ref_stars.iter().map(|s| s.pos).collect();
        let target_positions: Vec<DVec2> = target_stars.iter().map(|s| s.pos).collect();

        self.register_positions(&ref_positions, &target_positions)
    }

    /// Register target to reference using star positions as DVec2.
    ///
    /// This is useful when you have pre-extracted positions or for testing.
    /// Positions should be sorted by brightness (brightest first) for best results.
    ///
    /// # Arguments
    ///
    /// * `ref_positions` - Star positions in the reference image
    /// * `target_positions` - Star positions in the target image
    ///
    /// # Returns
    ///
    /// Registration result containing the transformation and quality metrics.
    pub fn register_positions(
        &self,
        ref_positions: &[DVec2],
        target_positions: &[DVec2],
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
        let max_stars = self.config.triangle.max_stars;
        let ref_stars = if self.config.use_spatial_distribution {
            select_spatially_distributed(ref_positions, max_stars, self.config.spatial_grid_size)
        } else {
            ref_positions.iter().take(max_stars).copied().collect()
        };
        let target_stars = if self.config.use_spatial_distribution {
            select_spatially_distributed(target_positions, max_stars, self.config.spatial_grid_size)
        } else {
            target_positions.iter().take(max_stars).copied().collect()
        };

        // Step 1: Triangle matching to find star correspondences
        let matches = match_triangles(&ref_stars, &target_stars, &self.config.triangle);

        if matches.len() < self.config.min_matched_stars {
            return Err(RegistrationError::NoMatchingPatterns);
        }

        // Step 2: Estimate transform — Auto mode tries Similarity first, upgrades if needed
        let result = if self.config.transform_type == TransformType::Auto {
            // Auto: start with Similarity, upgrade to Homography if residuals are high
            let sim_result = self.estimate_and_refine(
                &ref_stars,
                &target_stars,
                &matches,
                TransformType::Similarity,
            );

            const AUTO_UPGRADE_THRESHOLD: f64 = 0.5;

            match sim_result {
                Ok(result) if result.rms_error <= AUTO_UPGRADE_THRESHOLD => Ok(result),
                _ => {
                    // Similarity was insufficient or failed — try Homography
                    self.estimate_and_refine(
                        &ref_stars,
                        &target_stars,
                        &matches,
                        TransformType::Homography,
                    )
                }
            }
        } else {
            self.estimate_and_refine(
                &ref_stars,
                &target_stars,
                &matches,
                self.config.transform_type,
            )
        }?;

        let result = result.with_elapsed(start.elapsed().as_secs_f64() * 1000.0);

        // Check accuracy
        if result.rms_error > self.config.max_residual_pixels {
            return Err(RegistrationError::AccuracyTooLow {
                rms_error: result.rms_error,
                max_allowed: self.config.max_residual_pixels,
            });
        }

        Ok(result)
    }

    /// Run RANSAC, guided matching, optional SIP correction, and compute residuals.
    ///
    /// This is the core estimation pipeline used by `register_positions()`.
    /// For `TransformType::Auto`, this is called once with Similarity and potentially
    /// again with Homography if residuals are too high.
    fn estimate_and_refine(
        &self,
        ref_stars: &[DVec2],
        target_stars: &[DVec2],
        matches: &[StarMatch],
        transform_type: TransformType,
    ) -> Result<RegistrationResult, RegistrationError> {
        // RANSAC with progressive sampling using match confidences
        let ransac = RansacEstimator::new(self.config.ransac.clone());
        let ransac_result = ransac
            .estimate_with_matches(matches, ref_stars, target_stars, transform_type)
            .ok_or(RegistrationError::RansacFailed {
                reason: RansacFailureReason::NoInliersFound,
                iterations: self.config.ransac.max_iterations,
                best_inlier_count: 0,
            })?;

        // Get inlier matches — ransac_result.inliers are indices into `matches`
        let inlier_matches: Vec<_> = ransac_result
            .inliers
            .iter()
            .map(|&i| (matches[i].ref_idx, matches[i].target_idx))
            .collect();

        // Guided matching — recover additional matches missed by triangles
        let (transform, inlier_matches) = recover_matches(
            ref_stars,
            target_stars,
            &ransac_result.transform,
            &inlier_matches,
            self.config.ransac.inlier_threshold,
            transform_type,
        );

        // Optional SIP distortion correction on residuals
        let sip_correction = if self.config.sip.enabled {
            let inlier_ref: Vec<DVec2> =
                inlier_matches.iter().map(|&(r, _)| ref_stars[r]).collect();
            let inlier_target: Vec<DVec2> = inlier_matches
                .iter()
                .map(|&(_, t)| target_stars[t])
                .collect();

            let sip_config = SipConfig {
                order: self.config.sip.order,
                reference_point: None, // Auto-compute centroid
            };

            SipPolynomial::fit_from_transform(&inlier_ref, &inlier_target, &transform, &sip_config)
        } else {
            None
        };

        // Compute residuals for inliers (with SIP correction if available)
        let residuals: Vec<f64> = inlier_matches
            .iter()
            .map(|&(r, t)| {
                let ref_pos = ref_stars[r];
                let target_pos = target_stars[t];
                let corrected_r = match &sip_correction {
                    Some(sip) => sip.correct(ref_pos),
                    None => ref_pos,
                };
                let p = transform.apply(corrected_r);
                (p - target_pos).length()
            })
            .collect();

        let mut result = RegistrationResult::new(transform, inlier_matches, residuals);
        result.sip_correction = sip_correction;
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
        ref_stars: &[DVec2],
        target_stars: &[DVec2],
    ) -> Result<RegistrationResult, RegistrationError> {
        let start = Instant::now();

        // Validate dimensions
        if ref_image.len() != width * height || target_image.len() != width * height {
            return Err(RegistrationError::DimensionMismatch);
        }

        // Phase correlation for coarse alignment
        let correlator = PhaseCorrelator::new(width, height, self.config.phase_correlation.clone());

        let phase_result = correlator.correlate(ref_image, target_image);

        // Apply coarse translation to target stars if phase correlation succeeded
        let adjusted_target_stars: Vec<_> = if let Some(ref pr) = phase_result {
            let offset = pr.translation;
            target_stars.iter().map(|p| *p - offset).collect()
        } else {
            target_stars.to_vec()
        };

        // Now do star-based registration
        let mut result = self.register_positions(ref_stars, &adjusted_target_stars)?;

        // If we used phase correlation, compose the transforms
        if let Some(pr) = phase_result {
            let phase_transform = Transform::translation(pr.translation);
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

/// Warp target image to align with reference (raw pixel data, single channel).
///
/// Internal helper used by [`warp_to_reference_image`].
fn warp_to_reference(
    target_image: &Buffer2<f32>,
    transform: &Transform,
    method: InterpolationMethod,
) -> Buffer2<f32> {
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
        target_image.width(),
        target_image.height(),
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
    transform: &Transform,
    method: InterpolationMethod,
) -> crate::AstroImage {
    use rayon::prelude::*;

    let width = target.width();
    let height = target.height();
    let channels = target.channels();

    // Warp each channel in parallel using planar access
    let warped_channels: Vec<Buffer2<f32>> = (0..channels)
        .into_par_iter()
        .map(|c| {
            let channel_data = target.channel(c);
            let channel = Buffer2::new(width, height, channel_data.to_vec());
            warp_to_reference(&channel, transform, method)
        })
        .collect();

    // Interleave channels back together for AstroImage constructor
    let mut warped_pixels = vec![0.0f32; width * height * channels];
    for (c, channel_data) in warped_channels.iter().enumerate() {
        for (i, &val) in channel_data.iter().enumerate() {
            warped_pixels[i * channels + c] = val;
        }
    }

    let mut result = crate::AstroImage::from_pixels(
        ImageDimensions::new(width, height, channels),
        warped_pixels,
    );
    result.metadata = target.metadata.clone();
    result
}

/// Quick registration using default settings with position vectors.
///
/// Suitable for well-aligned images with good star coverage.
/// Returns only the transformation matrix. For full registration result
/// with quality metrics, use `quick_register_stars`.
///
/// # Arguments
///
/// * `ref_stars` - Reference star positions
/// * `target_stars` - Target star positions
///
/// # Returns
///
/// The transformation matrix that maps reference to target coordinates.
///
/// # Example
/// ```rust,ignore
/// use glam::DVec2;
/// use lumos::{quick_register, warp_to_reference_image, InterpolationMethod, AstroImage};
///
/// // Star positions detected from both images
/// let ref_positions = vec![DVec2::new(100.0, 200.0), DVec2::new(300.0, 150.0), /* ... */];
/// let target_positions = vec![DVec2::new(102.0, 198.0), DVec2::new(302.0, 148.0), /* ... */];
///
/// // Get the transformation matrix
/// let transform = quick_register(&ref_positions, &target_positions)?;
///
/// // Warp target image to align with reference
/// let aligned = warp_to_reference_image(&target_image, &transform, InterpolationMethod::Lanczos3);
/// ```
pub fn quick_register(
    ref_stars: &[DVec2],
    target_stars: &[DVec2],
) -> Result<Transform, RegistrationError> {
    let config = RegistrationConfig {
        transform_type: TransformType::Similarity,
        min_matched_stars: 4,
        triangle: crate::registration::config::TriangleMatchConfig {
            max_stars: 100,
            ..crate::registration::config::TriangleMatchConfig::default()
        },
        ransac: crate::registration::config::RansacConfig {
            max_iterations: 500,
            ..crate::registration::config::RansacConfig::default()
        },
        ..Default::default()
    };

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
    let config = RegistrationConfig {
        transform_type: TransformType::Affine,
        min_matched_stars: 4,
        max_residual_pixels: 5.0,
        triangle: crate::registration::config::TriangleMatchConfig {
            max_stars: 100,
            ..crate::registration::config::TriangleMatchConfig::default()
        },
        ..Default::default()
    };

    Registrator::new(config).register_stars(ref_stars, target_stars)
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
}

impl MultiScaleRegistrator {
    /// Create a new multi-scale registrator.
    ///
    /// The `config.multi_scale` field must be `Some` with a valid `MultiScaleConfig`.
    pub fn new(config: RegistrationConfig) -> Self {
        assert!(
            config.multi_scale.is_some(),
            "MultiScaleRegistrator requires config.multi_scale to be Some"
        );
        config.validate();
        Self { config }
    }

    fn multiscale_config(&self) -> &MultiScaleConfig {
        self.config.multi_scale.as_ref().unwrap()
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
        ref_stars: &[DVec2],
        target_stars: &[DVec2],
        image_width: usize,
        image_height: usize,
    ) -> Result<RegistrationResult, RegistrationError> {
        let start = Instant::now();
        let ms = self.multiscale_config();

        // Calculate actual number of levels based on image size
        let min_dim = image_width.min(image_height);
        let max_levels = ((min_dim as f64 / ms.min_dimension as f64).log2()
            / ms.scale_factor.log2())
        .floor() as usize
            + 1;
        let num_levels = ms.levels.min(max_levels).max(1);

        if num_levels == 1 {
            // Single level - just do normal registration
            return Registrator::new(self.config.clone())
                .register_positions(ref_stars, target_stars);
        }

        // Start from coarsest level and work down
        let mut current_transform = Transform::identity();
        let mut last_result: Option<RegistrationResult> = None;

        for level in (0..num_levels).rev() {
            let scale = ms.scale_factor.powi(level as i32);

            // Scale star positions to this level
            let scaled_ref: Vec<DVec2> = ref_stars.iter().map(|p| *p / scale).collect();

            // Apply current transform estimate to target stars, then scale
            let adjusted_target: Vec<DVec2> = target_stars
                .iter()
                .map(|p| {
                    // Apply inverse of current estimate to pre-align
                    let inv = current_transform.inverse();
                    let a = inv.apply(*p);
                    a / scale
                })
                .collect();

            // Use relaxed config for coarse levels, stricter for fine
            let level_config = if level > 0 {
                let mut c = self.config.clone();
                c.ransac.inlier_threshold = self.config.ransac.inlier_threshold * scale.sqrt();
                c.triangle.ratio_tolerance = self.config.triangle.ratio_tolerance * 1.5;
                c.ransac.max_iterations = self.config.ransac.max_iterations / 2;
                c.max_residual_pixels = self.config.max_residual_pixels * scale;
                c
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
        ref_stars: &[DVec2],
        target_stars: &[DVec2],
    ) -> Result<RegistrationResult, RegistrationError> {
        let start = Instant::now();
        let ms = self.multiscale_config();

        if ref_image.len() != width * height || target_image.len() != width * height {
            return Err(RegistrationError::DimensionMismatch);
        }

        // Calculate actual number of levels
        let min_dim = width.min(height);
        let max_levels = ((min_dim as f64 / ms.min_dimension as f64).log2()
            / ms.scale_factor.log2())
        .floor() as usize
            + 1;
        let num_levels = ms.levels.min(max_levels).max(1);

        let mut current_transform = Transform::identity();

        // Build image pyramid
        let ref_pyramid = build_pyramid(ref_image, width, height, num_levels, ms.scale_factor);
        let target_pyramid =
            build_pyramid(target_image, width, height, num_levels, ms.scale_factor);

        // Process from coarse to fine
        for level in (0..num_levels).rev() {
            let scale = ms.scale_factor.powi(level as i32);
            let (level_ref, level_width, level_height) = &ref_pyramid[level];
            let (level_target, _, _) = &target_pyramid[level];

            // At coarse levels, use phase correlation if enabled
            if level > 0 && ms.use_phase_correlation {
                let correlator = PhaseCorrelator::new(
                    *level_width,
                    *level_height,
                    self.config.phase_correlation.clone(),
                );

                if let Some(pr) = correlator.correlate(level_ref, level_target) {
                    // Scale translation to full resolution
                    let phase_transform = Transform::translation(pr.translation * scale);
                    current_transform = phase_transform.compose(&current_transform);
                }
            }

            // Scale star positions and do star matching
            let scaled_ref: Vec<DVec2> = ref_stars.iter().map(|p| *p / scale).collect();

            let adjusted_target: Vec<DVec2> = target_stars
                .iter()
                .map(|p| {
                    let inv = current_transform.inverse();
                    inv.apply_inverse(*p) / scale
                })
                .collect();

            let level_config = if level > 0 {
                let mut c = self.config.clone();
                c.ransac.inlier_threshold = self.config.ransac.inlier_threshold * scale.sqrt();
                c.triangle.ratio_tolerance = self.config.triangle.ratio_tolerance * 1.5;
                c.ransac.max_iterations = self.config.ransac.max_iterations / 2;
                c.max_residual_pixels = self.config.max_residual_pixels * scale;
                c
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
        let adjusted_target: Vec<DVec2> = target_stars
            .iter()
            .map(|p| current_transform.inverse().apply_inverse(*p))
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
fn scale_transform(transform: &Transform, scale: f64) -> Transform {
    // For a transform T that works at scale s, the equivalent at scale 1 is:
    // Scale the translation by s, keep rotation/scale the same
    let mut data = transform.data;
    data[2] *= scale; // tx
    data[5] *= scale; // ty
    Transform::matrix(data, transform.transform_type)
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
/// * `stars` - Star positions, assumed sorted by brightness (brightest first)
/// * `max_stars` - Maximum number of stars to select
/// * `grid_size` - Number of grid cells in each dimension (grid_size × grid_size)
///
/// # Returns
///
/// Selected star positions with good spatial coverage.
fn select_spatially_distributed(stars: &[DVec2], max_stars: usize, grid_size: usize) -> Vec<DVec2> {
    if stars.is_empty() || max_stars == 0 {
        return Vec::new();
    }

    // Find bounding box of all stars
    let (min_x, max_x, min_y, max_y) = stars.iter().fold(
        (f64::MAX, f64::MIN, f64::MAX, f64::MIN),
        |(min_x, max_x, min_y, max_y), p| {
            (
                min_x.min(p.x),
                max_x.max(p.x),
                min_y.min(p.y),
                max_y.max(p.y),
            )
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
    let mut cells: Vec<Vec<DVec2>> = vec![Vec::new(); num_cells];

    for &p in stars {
        let cx = ((p.x - origin_x) / cell_width) as usize;
        let cy = ((p.y - origin_y) / cell_height) as usize;
        let cx = cx.min(grid_size - 1);
        let cy = cy.min(grid_size - 1);
        cells[cy * grid_size + cx].push(p);
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

/// Guided matching: recover additional star matches missed by triangle matching.
///
/// After RANSAC finds a good transform, projects each unmatched reference star
/// into target space and searches for the nearest unmatched target star within
/// the inlier threshold. Then re-estimates the transform using all matches.
///
/// Returns the updated transform and combined matches (original + recovered).
/// If no new matches are found or re-estimation fails, returns inputs unchanged.
fn recover_matches(
    ref_stars: &[DVec2],
    target_stars: &[DVec2],
    transform: &Transform,
    inlier_matches: &[(usize, usize)],
    inlier_threshold: f64,
    transform_type: TransformType,
) -> (Transform, Vec<(usize, usize)>) {
    let target_tree = match KdTree::build(target_stars) {
        Some(tree) => tree,
        None => return (transform.clone(), inlier_matches.to_vec()),
    };

    let matched_ref: std::collections::HashSet<usize> =
        inlier_matches.iter().map(|&(r, _)| r).collect();
    let matched_target: std::collections::HashSet<usize> =
        inlier_matches.iter().map(|&(_, t)| t).collect();

    let threshold_sq = inlier_threshold * inlier_threshold;
    let mut all_matches: Vec<(usize, usize)> = inlier_matches.to_vec();
    let mut newly_matched_targets = std::collections::HashSet::new();

    for (ref_idx, &ref_pos) in ref_stars.iter().enumerate() {
        if matched_ref.contains(&ref_idx) {
            continue;
        }

        let predicted = transform.apply(ref_pos);
        let nearest = target_tree.k_nearest(predicted, 1);

        if let Some(&(target_idx, dist_sq)) = nearest.first()
            && dist_sq <= threshold_sq
            && !matched_target.contains(&target_idx)
            && !newly_matched_targets.contains(&target_idx)
        {
            all_matches.push((ref_idx, target_idx));
            newly_matched_targets.insert(target_idx);
        }
    }

    if all_matches.len() == inlier_matches.len() {
        return (transform.clone(), all_matches);
    }

    let all_ref: Vec<DVec2> = all_matches.iter().map(|&(r, _)| ref_stars[r]).collect();
    let all_target: Vec<DVec2> = all_matches.iter().map(|&(_, t)| target_stars[t]).collect();

    match estimate_transform(&all_ref, &all_target, transform_type) {
        Some(new_transform) => (new_transform, all_matches),
        None => (transform.clone(), inlier_matches.to_vec()),
    }
}
