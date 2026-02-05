//! Full image registration pipeline.
//!
//! This module provides the internal orchestration for registering astronomical images,
//! combining triangle matching, RANSAC, and image warping.

mod result;

pub use result::{RansacFailureReason, RegistrationError, RegistrationResult};

use glam::DVec2;
use std::time::Instant;

use crate::ImageDimensions;
use crate::common::Buffer2;
use crate::registration::{
    Config, InterpolationMethod, Transform, TransformType,
    distortion::{SipConfig, SipPolynomial},
    interpolation::warp_image,
    ransac::{RansacEstimator, RansacParams, estimate_transform},
    spatial::KdTree,
    triangle::{PointMatch, TriangleParams, match_triangles},
};
use crate::star_detection::Star;

#[cfg(test)]
mod tests;

/// Image registrator that aligns target images to a reference.
#[derive(Debug, Default)]
pub struct Registrator {
    config: Config,
}

impl Registrator {
    /// Create a new registrator with the given configuration.
    pub fn new(config: Config) -> Self {
        config.validate();
        Self { config }
    }

    /// Register target to reference using pre-detected stars.
    ///
    /// Stars should be sorted by brightness (flux) in descending order.
    pub fn register_stars(
        &self,
        ref_stars: &[Star],
        target_stars: &[Star],
    ) -> Result<RegistrationResult, RegistrationError> {
        let ref_positions: Vec<DVec2> = ref_stars.iter().map(|s| s.pos).collect();
        let target_positions: Vec<DVec2> = target_stars.iter().map(|s| s.pos).collect();
        self.register_positions(&ref_positions, &target_positions)
    }

    /// Register target to reference using star positions as DVec2.
    pub fn register_positions(
        &self,
        ref_positions: &[DVec2],
        target_positions: &[DVec2],
    ) -> Result<RegistrationResult, RegistrationError> {
        let start = Instant::now();

        // Validate input
        if ref_positions.len() < self.config.min_stars {
            return Err(RegistrationError::InsufficientStars {
                found: ref_positions.len(),
                required: self.config.min_stars,
            });
        }
        if target_positions.len() < self.config.min_stars {
            return Err(RegistrationError::InsufficientStars {
                found: target_positions.len(),
                required: self.config.min_stars,
            });
        }

        // Select stars for matching (take brightest N)
        let ref_stars: Vec<DVec2> = ref_positions
            .iter()
            .take(self.config.max_stars)
            .copied()
            .collect();
        let target_stars: Vec<DVec2> = target_positions
            .iter()
            .take(self.config.max_stars)
            .copied()
            .collect();

        // Triangle matching
        let triangle_params = TriangleParams {
            ratio_tolerance: self.config.ratio_tolerance,
            min_votes: self.config.min_votes,
            check_orientation: self.config.check_orientation,
        };
        let matches = match_triangles(&ref_stars, &target_stars, &triangle_params);

        if matches.len() < self.config.min_matches {
            return Err(RegistrationError::NoMatchingPatterns);
        }

        // RANSAC estimation
        let result = if self.config.transform_type == TransformType::Auto {
            let sim_result = self.estimate_and_refine(
                &ref_stars,
                &target_stars,
                &matches,
                TransformType::Similarity,
            );

            const AUTO_UPGRADE_THRESHOLD: f64 = 0.5;

            match sim_result {
                Ok(result) if result.rms_error <= AUTO_UPGRADE_THRESHOLD => Ok(result),
                _ => self.estimate_and_refine(
                    &ref_stars,
                    &target_stars,
                    &matches,
                    TransformType::Homography,
                ),
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

        if result.rms_error > self.config.max_rms_error {
            return Err(RegistrationError::AccuracyTooLow {
                rms_error: result.rms_error,
                max_allowed: self.config.max_rms_error,
            });
        }

        Ok(result)
    }

    fn estimate_and_refine(
        &self,
        ref_stars: &[DVec2],
        target_stars: &[DVec2],
        matches: &[PointMatch],
        transform_type: TransformType,
    ) -> Result<RegistrationResult, RegistrationError> {
        let ransac_params = RansacParams {
            max_iterations: self.config.ransac_iterations,
            inlier_threshold: self.config.inlier_threshold,
            confidence: self.config.confidence,
            min_inlier_ratio: self.config.min_inlier_ratio,
            seed: self.config.seed,
            use_local_optimization: self.config.local_optimization,
            lo_max_iterations: self.config.lo_iterations,
            max_rotation: self.config.max_rotation,
            scale_range: self.config.scale_range,
        };

        let ransac = RansacEstimator::new(ransac_params);
        let ransac_result = ransac
            .estimate(matches, ref_stars, target_stars, transform_type)
            .ok_or(RegistrationError::RansacFailed {
                reason: RansacFailureReason::NoInliersFound,
                iterations: self.config.ransac_iterations,
                best_inlier_count: 0,
            })?;

        let inlier_matches: Vec<_> = ransac_result
            .inliers
            .iter()
            .map(|&i| (matches[i].ref_idx, matches[i].target_idx))
            .collect();

        let (transform, inlier_matches) = recover_matches(
            ref_stars,
            target_stars,
            &ransac_result.transform,
            &inlier_matches,
            self.config.inlier_threshold,
            transform_type,
        );

        let sip_correction = if self.config.sip_enabled {
            let inlier_ref: Vec<DVec2> =
                inlier_matches.iter().map(|&(r, _)| ref_stars[r]).collect();
            let inlier_target: Vec<DVec2> = inlier_matches
                .iter()
                .map(|&(_, t)| target_stars[t])
                .collect();

            let sip_config = SipConfig {
                order: self.config.sip_order,
                reference_point: None,
            };

            SipPolynomial::fit_from_transform(&inlier_ref, &inlier_target, &transform, &sip_config)
        } else {
            None
        };

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

/// Warp an AstroImage to align with reference.
pub fn warp_to_reference_image(
    target: &crate::AstroImage,
    transform: &Transform,
    method: InterpolationMethod,
) -> crate::AstroImage {
    use rayon::prelude::*;

    let width = target.width();
    let height = target.height();
    let channels = target.channels();

    let warped_channels: Vec<Buffer2<f32>> = (0..channels)
        .into_par_iter()
        .map(|c| {
            let channel_data = target.channel(c);
            let channel = Buffer2::new(width, height, channel_data.to_vec());
            let inverse = transform.inverse();
            warp_image(&channel, width, height, &inverse, method, 0.0, true, false)
        })
        .collect();

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
