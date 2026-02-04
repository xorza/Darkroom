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
    config::{InterpolationMethod, RegistrationConfig, WarpConfig},
    distortion::{SipConfig, SipPolynomial},
    interpolation::warp_image,
    ransac::{RansacEstimator, estimate_transform},
    spatial::KdTree,
    transform::Transform,
    transform::TransformType,
    triangle::{PointMatch, match_triangles},
};
use crate::star_detection::Star;

#[cfg(test)]
mod tests;

/// Image registrator that aligns target images to a reference.
#[derive(Debug, Default)]
pub struct Registrator {
    config: RegistrationConfig,
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
        matches: &[PointMatch],
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
        None => return (*transform, inlier_matches.to_vec()),
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

        if let Some(nearest_neighbor) = nearest.first()
            && nearest_neighbor.dist_sq <= threshold_sq
            && !matched_target.contains(&nearest_neighbor.index)
            && !newly_matched_targets.contains(&nearest_neighbor.index)
        {
            all_matches.push((ref_idx, nearest_neighbor.index));
            newly_matched_targets.insert(nearest_neighbor.index);
        }
    }

    if all_matches.len() == inlier_matches.len() {
        return (*transform, all_matches);
    }

    let all_ref: Vec<DVec2> = all_matches.iter().map(|&(r, _)| ref_stars[r]).collect();
    let all_target: Vec<DVec2> = all_matches.iter().map(|&(_, t)| target_stars[t]).collect();

    match estimate_transform(&all_ref, &all_target, transform_type) {
        Some(new_transform) => (new_transform, all_matches),
        None => {
            all_matches.truncate(inlier_matches.len());
            (*transform, all_matches)
        }
    }
}
