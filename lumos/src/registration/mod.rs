//! Image registration module for astronomical image alignment.
//!
//! This module provides star-based image registration using triangle matching
//! and RANSAC for robust transformation estimation.
//!
//! # Quick Start
//!
//! ```ignore
//! use lumos::registration::{register, warp, Config};
//!
//! // Register stars from two images
//! let result = register(&ref_stars, &target_stars, &Config::default())?;
//! println!("Matched {} stars, RMS = {:.2}px", result.num_inliers, result.rms_error);
//!
//! // Warp target image in place to align with reference
//! let aligned = warp(target_image, &result.transform, &Config::default());
//! ```
//!
//! # Transformation Models
//!
//! | Type | DOF | Description |
//! |------|-----|-------------|
//! | Translation | 2 | X/Y offset only |
//! | Euclidean | 3 | Translation + rotation |
//! | Similarity | 4 | Translation + rotation + uniform scale |
//! | Affine | 6 | Handles shear and differential scaling |
//! | Homography | 8 | Full perspective transformation |
//! | Auto | - | Starts with Similarity, upgrades to Homography if needed |
//!
//! # Configuration Presets
//!
//! - [`Config::default()`] — Balanced settings for most astrophotography
//! - [`Config::fast()`] — Fewer iterations, bilinear interpolation
//! - [`Config::precise()`] — More iterations, SIP distortion correction
//! - [`Config::wide_field()`] — Homography + SIP for wide-field lenses
//! - [`Config::mosaic()`] — Allows larger rotations and scale differences

pub(crate) mod config;
pub(crate) mod distortion;
pub(crate) mod interpolation;
pub(crate) mod ransac;
mod result;
pub(crate) mod spatial;
pub(crate) mod transform;
pub(crate) mod triangle;

#[cfg(test)]
mod tests;

// === Primary Public API ===

// Configuration
pub use config::{Config, InterpolationMethod};

// Core types
pub use transform::{Transform, TransformType};

// Results and errors
pub use result::{RansacFailureReason, RegistrationError, RegistrationResult};

// Distortion (for users who need manual SIP access)
pub use distortion::SipPolynomial;

// === Top-Level Functions ===

use std::time::Instant;

use glam::DVec2;
use rayon::prelude::*;

use crate::AstroImage;
use crate::star_detection::Star;
use distortion::SipConfig;
use interpolation::warp_image;
use ransac::{RansacEstimator, RansacParams, estimate_transform};
use spatial::KdTree;
use triangle::{PointMatch, TriangleParams, match_triangles};

/// Register two sets of star positions.
///
/// This is the main entry point for image registration. It finds the geometric
/// transformation that maps reference star positions to target star positions.
///
/// Stars should be sorted by brightness (flux) in descending order for best results.
///
/// # Example
///
/// ```ignore
/// use lumos::registration::{register, Config, TransformType};
///
/// // With defaults
/// let result = register(&ref_stars, &target_stars, &Config::default())?;
///
/// // With custom config
/// let config = Config {
///     transform_type: TransformType::Similarity,
///     inlier_threshold: 3.0,
///     ..Config::default()
/// };
/// let result = register(&ref_stars, &target_stars, &config)?;
///
/// println!("Matched {} stars", result.num_inliers);
/// println!("RMS error: {:.2} pixels", result.rms_error);
/// ```
pub fn register(
    ref_stars: &[Star],
    target_stars: &[Star],
    config: &Config,
) -> Result<RegistrationResult, RegistrationError> {
    let ref_positions: Vec<DVec2> = ref_stars.iter().map(|s| s.pos).collect();
    let target_positions: Vec<DVec2> = target_stars.iter().map(|s| s.pos).collect();
    register_positions(&ref_positions, &target_positions, config)
}

/// Register using raw position vectors instead of Star structs.
///
/// Useful when you have pre-extracted positions or for testing.
/// Positions should be sorted by brightness (brightest first) for best results.
pub fn register_positions(
    ref_positions: &[DVec2],
    target_positions: &[DVec2],
    config: &Config,
) -> Result<RegistrationResult, RegistrationError> {
    config.validate();
    let start = Instant::now();

    // Validate input
    if ref_positions.len() < config.min_stars {
        return Err(RegistrationError::InsufficientStars {
            found: ref_positions.len(),
            required: config.min_stars,
        });
    }
    if target_positions.len() < config.min_stars {
        return Err(RegistrationError::InsufficientStars {
            found: target_positions.len(),
            required: config.min_stars,
        });
    }

    // Select stars for matching (take brightest N)
    let ref_stars: Vec<DVec2> = ref_positions
        .iter()
        .take(config.max_stars)
        .copied()
        .collect();
    let target_stars: Vec<DVec2> = target_positions
        .iter()
        .take(config.max_stars)
        .copied()
        .collect();

    // Triangle matching
    let triangle_params = TriangleParams {
        ratio_tolerance: config.ratio_tolerance,
        min_votes: config.min_votes,
        check_orientation: config.check_orientation,
    };
    let matches = match_triangles(&ref_stars, &target_stars, &triangle_params);

    if matches.len() < config.min_matches {
        return Err(RegistrationError::NoMatchingPatterns);
    }

    // RANSAC estimation
    let result = if config.transform_type == TransformType::Auto {
        let sim_result = estimate_and_refine(
            &ref_stars,
            &target_stars,
            &matches,
            TransformType::Similarity,
            config,
        );

        const AUTO_UPGRADE_THRESHOLD: f64 = 0.5;

        match sim_result {
            Ok(result) if result.rms_error <= AUTO_UPGRADE_THRESHOLD => Ok(result),
            _ => estimate_and_refine(
                &ref_stars,
                &target_stars,
                &matches,
                TransformType::Homography,
                config,
            ),
        }
    } else {
        estimate_and_refine(
            &ref_stars,
            &target_stars,
            &matches,
            config.transform_type,
            config,
        )
    }?;

    let result = result.with_elapsed(start.elapsed().as_secs_f64() * 1000.0);

    if result.rms_error > config.max_rms_error {
        return Err(RegistrationError::AccuracyTooLow {
            rms_error: result.rms_error,
            max_allowed: config.max_rms_error,
        });
    }

    Ok(result)
}

/// Warp an image to align with the reference frame, writing the result into an output image.
///
/// Applies the inverse transformation so the image aligns
/// pixel-for-pixel with the reference image.
///
/// # Arguments
/// * `image` - The source image to warp (consumed)
/// * `output` - The destination image where the warped result is written
/// * `transform` - The geometric transformation to apply
/// * `config` - Configuration for interpolation method
///
/// # Panics
/// Panics if the output image dimensions or channel count don't match the input.
///
/// # Example
///
/// ```ignore
/// use lumos::registration::{register, warp, Config};
///
/// let result = register(&ref_stars, &target_stars, &Config::default())?;
/// let mut aligned = target_image.clone();
/// warp(target_image, &mut aligned, &result.transform, &Config::default());
/// ```
pub fn warp(image: &AstroImage, output: &mut AstroImage, transform: &Transform, config: &Config) {
    assert_eq!(
        image.dimensions(),
        output.dimensions(),
        "Output dimensions must match input"
    );

    let method = config.interpolation;
    let inverse = transform.inverse();

    for c in 0..image.channels() {
        let input = image.channel(c);
        let output_buf = output.channel_mut(c);
        warp_image(input, output_buf, &inverse, method);
    }
}

// === Internal Functions ===

fn estimate_and_refine(
    ref_stars: &[DVec2],
    target_stars: &[DVec2],
    matches: &[PointMatch],
    transform_type: TransformType,
    config: &Config,
) -> Result<RegistrationResult, RegistrationError> {
    let ransac_params = RansacParams {
        max_iterations: config.ransac_iterations,
        inlier_threshold: config.inlier_threshold,
        confidence: config.confidence,
        min_inlier_ratio: config.min_inlier_ratio,
        seed: config.seed,
        use_local_optimization: config.local_optimization,
        lo_max_iterations: config.lo_iterations,
        max_rotation: config.max_rotation,
        scale_range: config.scale_range,
    };

    let ransac = RansacEstimator::new(ransac_params);
    let ransac_result = ransac
        .estimate(matches, ref_stars, target_stars, transform_type)
        .ok_or(RegistrationError::RansacFailed {
            reason: RansacFailureReason::NoInliersFound,
            iterations: config.ransac_iterations,
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
        config.inlier_threshold,
        transform_type,
    );

    let sip_correction = if config.sip_enabled {
        let inlier_ref: Vec<DVec2> = inlier_matches.iter().map(|&(r, _)| ref_stars[r]).collect();
        let inlier_target: Vec<DVec2> = inlier_matches
            .iter()
            .map(|&(_, t)| target_stars[t])
            .collect();

        let sip_config = SipConfig {
            order: config.sip_order,
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

pub(crate) fn recover_matches(
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

// === Internal Re-exports (for submodules) ===

pub(crate) use ransac::RansacResult;
