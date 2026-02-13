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
//! let aligned = warp(target_image, &result.warp_transform(), &Config::default());
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
pub use transform::{Transform, TransformType, WarpTransform};

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
/// The RANSAC `max_sigma` parameter is automatically derived from the median FWHM
/// of the input stars, providing optimal noise tolerance for the seeing conditions.
///
/// # Example
///
/// ```ignore
/// use lumos::registration::{register, Config, TransformType};
///
/// // With defaults (max_sigma auto-derived from star FWHM)
/// let result = register(&ref_stars, &target_stars, &Config::default())?;
///
/// // With custom config
/// let config = Config {
///     transform_type: TransformType::Similarity,
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
    config.validate();
    let start = Instant::now();

    // Validate input
    if ref_stars.len() < config.min_stars {
        return Err(RegistrationError::InsufficientStars {
            found: ref_stars.len(),
            required: config.min_stars,
        });
    }
    if target_stars.len() < config.min_stars {
        return Err(RegistrationError::InsufficientStars {
            found: target_stars.len(),
            required: config.min_stars,
        });
    }

    // Derive max_sigma from median FWHM for optimal noise tolerance
    let median_fwhm = median_fwhm(ref_stars, target_stars);
    let max_sigma = (median_fwhm * 0.5).max(0.5);

    // Select stars for matching (take brightest N)
    let ref_positions: Vec<DVec2> = ref_stars
        .iter()
        .take(config.max_stars)
        .map(|s| s.pos)
        .collect();
    let target_positions: Vec<DVec2> = target_stars
        .iter()
        .take(config.max_stars)
        .map(|s| s.pos)
        .collect();

    // Triangle matching
    let triangle_params = TriangleParams {
        ratio_tolerance: config.ratio_tolerance,
        min_votes: config.min_votes,
        check_orientation: config.check_orientation,
    };
    let matches = match_triangles(&ref_positions, &target_positions, &triangle_params);

    if matches.len() < config.min_matches {
        return Err(RegistrationError::NoMatchingPatterns);
    }

    // RANSAC estimation
    let result = if config.transform_type == TransformType::Auto {
        let sim_result = estimate_and_refine(
            &ref_positions,
            &target_positions,
            &matches,
            TransformType::Similarity,
            max_sigma,
            config,
        );

        const AUTO_UPGRADE_THRESHOLD: f64 = 0.5;

        match sim_result {
            Ok(result) if result.rms_error <= AUTO_UPGRADE_THRESHOLD => Ok(result),
            _ => estimate_and_refine(
                &ref_positions,
                &target_positions,
                &matches,
                TransformType::Homography,
                max_sigma,
                config,
            ),
        }
    } else {
        estimate_and_refine(
            &ref_positions,
            &target_positions,
            &matches,
            config.transform_type,
            max_sigma,
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

/// Compute the median FWHM from two sets of stars.
fn median_fwhm(ref_stars: &[Star], target_stars: &[Star]) -> f64 {
    let mut fwhms: Vec<f32> = ref_stars
        .iter()
        .chain(target_stars.iter())
        .map(|s| s.fwhm)
        .collect();

    fwhms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    fwhms[fwhms.len() / 2] as f64
}

/// Warp an image to align with the reference frame, writing the result into an output image.
///
/// The `WarpTransform` bundles the linear transform with optional SIP distortion
/// correction. Use `result.warp_transform()` to obtain one from a `RegistrationResult`,
/// or `WarpTransform::new(transform)` for a plain transform.
///
/// # Arguments
/// * `image` - The source (target) image to warp
/// * `output` - The destination image where the warped result is written
/// * `warp_transform` - Combined transform + optional SIP correction
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
/// warp(&target_image, &mut aligned, &result.warp_transform(), &Config::default());
/// ```
pub fn warp(
    image: &AstroImage,
    output: &mut AstroImage,
    warp_transform: &WarpTransform,
    config: &Config,
) {
    assert_eq!(
        image.dimensions(),
        output.dimensions(),
        "Output dimensions must match input"
    );

    let params = interpolation::WarpParams {
        method: config.interpolation,
        border_value: config.border_value,
    };

    for c in 0..image.channels() {
        let input = image.channel(c);
        let output_buf = output.channel_mut(c);
        warp_image(input, output_buf, warp_transform, &params);
    }
}

// === Internal Functions ===

fn estimate_and_refine(
    ref_stars: &[DVec2],
    target_stars: &[DVec2],
    matches: &[PointMatch],
    transform_type: TransformType,
    max_sigma: f64,
    config: &Config,
) -> Result<RegistrationResult, RegistrationError> {
    let ransac_params = RansacParams {
        max_iterations: config.ransac_iterations,
        max_sigma,
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

    // Effective threshold for match recovery: ~3 * max_sigma (χ² quantile)
    let effective_threshold = max_sigma * 3.03;
    let (transform, inlier_matches) = recover_matches(
        ref_stars,
        target_stars,
        &ransac_result.transform,
        &inlier_matches,
        effective_threshold,
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
            reference_point: config.sip_reference_point,
            ..Default::default()
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

#[cfg(test)]
mod fwhm_tests {
    use super::*;

    fn make_star(fwhm: f32) -> Star {
        Star {
            pos: DVec2::ZERO,
            flux: 1000.0,
            fwhm,
            eccentricity: 0.0,
            snr: 100.0,
            peak: 1.0,
            sharpness: 0.5,
            roundness1: 0.0,
            roundness2: 0.0,
            laplacian_snr: 50.0,
        }
    }

    #[test]
    fn test_median_fwhm_basic() {
        let ref_stars = vec![make_star(2.0), make_star(3.0), make_star(4.0)];
        let target_stars = vec![make_star(2.5), make_star(3.5)];
        // Combined: [2.0, 2.5, 3.0, 3.5, 4.0] -> median = 3.0
        let median = median_fwhm(&ref_stars, &target_stars);
        assert!((median - 3.0).abs() < 0.01);
    }

    #[test]
    fn test_median_fwhm_single_set() {
        let ref_stars = vec![make_star(1.5), make_star(2.5), make_star(3.5)];
        let target_stars = vec![];
        // Combined: [1.5, 2.5, 3.5] -> median = 2.5
        let median = median_fwhm(&ref_stars, &target_stars);
        assert!((median - 2.5).abs() < 0.01);
    }

    #[test]
    fn test_max_sigma_from_fwhm() {
        // FWHM = 3.0 -> max_sigma = 3.0 * 0.5 = 1.5
        let fwhm: f64 = 3.0;
        let max_sigma = (fwhm * 0.5).max(0.5);
        assert!((max_sigma - 1.5).abs() < 0.01);

        // FWHM = 0.8 -> max_sigma = max(0.4, 0.5) = 0.5 (floor)
        let fwhm: f64 = 0.8;
        let max_sigma = (fwhm * 0.5).max(0.5);
        assert!((max_sigma - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_max_sigma_typical_seeing() {
        // Typical ground seeing: FWHM = 2.0-4.0 pixels
        // FWHM = 2.0 -> max_sigma = 1.0 (~3px effective threshold)
        // FWHM = 4.0 -> max_sigma = 2.0 (~6px effective threshold)
        let ref_stars = vec![make_star(2.0), make_star(2.5), make_star(3.0)];
        let target_stars = vec![make_star(2.2), make_star(2.8)];

        let median = median_fwhm(&ref_stars, &target_stars);
        let max_sigma = (median * 0.5).max(0.5);

        // Median of [2.0, 2.2, 2.5, 2.8, 3.0] = 2.5
        // max_sigma = 2.5 * 0.5 = 1.25
        assert!((median - 2.5).abs() < 0.01);
        assert!((max_sigma - 1.25).abs() < 0.01);
    }
}
