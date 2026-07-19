//! Image registration module for astronomical image alignment.
//!
//! This module provides star-based image registration using triangle matching
//! and RANSAC for robust transformation estimation.
//!
//! # Quick Start
//!
//! ```ignore
//! use lumos::{RegistrationConfig, register, warp};
//!
//! // Register stars from two images
//! let result = register(&ref_stars, &target_stars, &RegistrationConfig::default())?;
//! println!("Matched {} stars, RMS = {:.2}px", result.num_inliers, result.rms_error);
//!
//! // Warp target image in place to align with reference
//! let config = RegistrationConfig::default();
//! let aligned = warp(target_image, &result.warp_transform(), &config.warp);
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
//! | Auto | - | Ladder Euclidean → Similarity → Affine → Homography; first within 0.5px RMS wins |
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
pub(crate) mod resample;
pub(crate) mod result;
pub(crate) mod spatial;
pub(crate) mod transform;
pub(crate) mod triangle;

#[cfg(test)]
mod bench;
#[cfg(all(test, feature = "real-data"))]
mod real_data_tests;
#[cfg(test)]
mod synthetic_tests;

use config::Config;
use distortion::sip::SipPolynomial;
use result::{RansacFailureReason, RegistrationError, RegistrationResult, StarMatch};
use transform::{Transform, TransformType};

use std::time::Instant;

use glam::DVec2;

use crate::stacking::star_detection::star::Star;
use ransac::RansacEstimator;
use ransac::transforms::estimate_transform;
use spatial::KdTree;
use triangle::matching::match_triangles;
use triangle::voting::PointMatch;

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
/// # Panics
///
/// If `config` fails validation (see [`Config::validate`]).
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
    config.validate()?;
    let start = Instant::now();

    // Validate input — the gate is keyed to the transform model unless min_stars overrides it.
    let required_stars = config.matching.required_stars(config.transform_type);
    if ref_stars.len() < required_stars {
        return Err(RegistrationError::InsufficientStars {
            found: ref_stars.len(),
            required: required_stars,
        });
    }
    if target_stars.len() < required_stars {
        return Err(RegistrationError::InsufficientStars {
            found: target_stars.len(),
            required: required_stars,
        });
    }

    // Derive max_sigma from median FWHM for optimal noise tolerance
    let median_fwhm = median_fwhm(ref_stars, target_stars);
    let max_sigma = (median_fwhm * 0.5).max(0.5);

    // Select stars for matching (take brightest N)
    let ref_positions: Vec<DVec2> = ref_stars
        .iter()
        .take(config.matching.max_stars)
        .map(|s| s.pos)
        .collect();
    let target_positions: Vec<DVec2> = target_stars
        .iter()
        .take(config.matching.max_stars)
        .map(|s| s.pos)
        .collect();

    // Triangle matching
    let t0 = Instant::now();
    let matches = match_triangles(&ref_positions, &target_positions, &config.matching.triangle);
    let triangle_ms = t0.elapsed().as_secs_f64() * 1000.0;
    tracing::debug!(
        triangle_ms,
        num_matches = matches.len(),
        "Triangle matching complete"
    );

    if matches.len() < config.matching.min_matches {
        return Err(RegistrationError::NoMatchingPatterns);
    }

    // RANSAC estimation
    let result = if config.transform_type == TransformType::Auto {
        auto_ladder(
            &ref_positions,
            &target_positions,
            &matches,
            max_sigma,
            config,
        )
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

    fwhms.sort_by(|a, b| a.total_cmp(b));
    fwhms[fwhms.len() / 2] as f64
}

/// Maximum RMS (px) at which an `Auto` rung is accepted before escalating to a more general model.
const AUTO_UPGRADE_THRESHOLD: f64 = 0.5;

/// `Auto` model selection: estimate transforms from fewest to most degrees of freedom and accept
/// the first whose RMS clears [`AUTO_UPGRADE_THRESHOLD`] — the *simplest model that fits*, so the
/// alignment isn't overfit to star-centroid noise (every extra DOF soaks up noise and generalizes
/// worse). Falls through to the most general model (Homography) when no simpler rung clears the bar;
/// the caller's `max_rms_error` gate then has the final say on that result.
///
/// The ladder is Euclidean → Similarity → Affine → Homography (rigid → +scale → +shear →
/// projective); earlier this only tried Similarity then jumped straight to Homography, so same-scale
/// rigid sets were fit with a needless scale DOF and mild differential distortion overshot to the
/// full projective model.
fn auto_ladder(
    ref_positions: &[DVec2],
    target_positions: &[DVec2],
    matches: &[PointMatch],
    max_sigma: f64,
    config: &Config,
) -> Result<RegistrationResult, RegistrationError> {
    for model in [
        TransformType::Euclidean,
        TransformType::Similarity,
        TransformType::Affine,
    ] {
        if let Ok(result) = estimate_and_refine(
            ref_positions,
            target_positions,
            matches,
            model,
            max_sigma,
            config,
        ) && result.rms_error <= AUTO_UPGRADE_THRESHOLD
        {
            return Ok(result);
        }
    }
    estimate_and_refine(
        ref_positions,
        target_positions,
        matches,
        TransformType::Homography,
        max_sigma,
        config,
    )
}

/// Run RANSAC estimation followed by match recovery and optional SIP fitting.
///
/// `transform_type` is passed separately from `config.transform_type` because
/// the Auto resolution logic resolves to a concrete type before calling this.
fn estimate_and_refine(
    ref_stars: &[DVec2],
    target_stars: &[DVec2],
    matches: &[PointMatch],
    transform_type: TransformType,
    max_sigma: f64,
    config: &Config,
) -> Result<RegistrationResult, RegistrationError> {
    let t0 = Instant::now();
    let ransac = RansacEstimator::new(config.ransac.clone(), max_sigma);
    let ransac_result = ransac
        .estimate(matches, ref_stars, target_stars, transform_type)
        .ok_or(RegistrationError::RansacFailed {
            reason: RansacFailureReason::NoInliersFound,
            iterations: config.ransac.max_iterations,
            best_inlier_count: 0,
        })?;
    let ransac_ms = t0.elapsed().as_secs_f64() * 1000.0;

    let inlier_matches: Vec<_> = ransac_result
        .inliers
        .iter()
        .map(|&i| StarMatch {
            reference: matches[i].ref_idx,
            target: matches[i].target_idx,
        })
        .collect();

    // Effective threshold for match recovery: ~3 * max_sigma (χ² quantile)
    let effective_threshold = max_sigma * 3.03;
    let t0 = Instant::now();
    let RecoveredMatches {
        transform,
        matches: inlier_matches,
    } = recover_matches(
        ref_stars,
        target_stars,
        &ransac_result.transform,
        &inlier_matches,
        effective_threshold,
        transform_type,
    );
    let recovery_ms = t0.elapsed().as_secs_f64() * 1000.0;

    let t0 = Instant::now();
    let sip_fit = if let Some(sip_config) = &config.sip {
        let inlier_ref: Vec<DVec2> = inlier_matches
            .iter()
            .map(|star_match| ref_stars[star_match.reference])
            .collect();
        let inlier_target: Vec<DVec2> = inlier_matches
            .iter()
            .map(|star_match| target_stars[star_match.target])
            .collect();

        Some(SipPolynomial::fit_from_transform(
            &inlier_ref,
            &inlier_target,
            &transform,
            sip_config,
        )?)
    } else {
        None
    };

    let sip_polynomial = sip_fit.as_ref().map(|r| &r.polynomial);

    let residuals: Vec<f64> = inlier_matches
        .iter()
        .map(|star_match| {
            let ref_pos = ref_stars[star_match.reference];
            let target_pos = target_stars[star_match.target];
            let corrected_r = match sip_polynomial {
                Some(sip) => sip.correct(ref_pos),
                None => ref_pos,
            };
            let p = transform.apply(corrected_r);
            (p - target_pos).length()
        })
        .collect();

    let sip_ms = t0.elapsed().as_secs_f64() * 1000.0;
    tracing::debug!(
        ransac_ms,
        recovery_ms,
        sip_ms,
        ransac_inliers = ransac_result.inliers.len(),
        "Registration sub-step timing"
    );

    let mut result = RegistrationResult::new(transform, inlier_matches, residuals);
    result.sip_fit = sip_fit;
    Ok(result)
}

/// Maximum iterations for iterative match recovery.
/// Convergence is typically reached in 2-3 passes; diminishing returns after that.
const RECOVERY_MAX_ITERATIONS: usize = 5;

#[derive(Debug)]
struct RecoveredMatches {
    transform: Transform,
    matches: Vec<StarMatch>,
}

fn recover_matches(
    ref_stars: &[DVec2],
    target_stars: &[DVec2],
    transform: &Transform,
    inlier_matches: &[StarMatch],
    inlier_threshold: f64,
    transform_type: TransformType,
) -> RecoveredMatches {
    let target_tree = match KdTree::build(target_stars) {
        Some(tree) => tree,
        None => {
            return RecoveredMatches {
                transform: *transform,
                matches: inlier_matches.to_vec(),
            };
        }
    };

    let threshold_sq = inlier_threshold * inlier_threshold;
    let mut current_transform = *transform;
    let mut current_matches = inlier_matches.to_vec();

    // Dense small-integer membership over [0, n) → bitmaps, not HashSets: no hashing,
    // no allocation per pass, and order-independent (deterministic).
    let mut matched_target = vec![false; target_stars.len()];
    let mut matched_ref = vec![false; ref_stars.len()];
    let mut newly_matched_targets = vec![false; target_stars.len()];

    for _ in 0..RECOVERY_MAX_ITERATIONS {
        let prev_count = current_matches.len();

        matched_target.fill(false);
        for star_match in &current_matches {
            matched_target[star_match.target] = true;
        }

        matched_ref.fill(false);
        for star_match in &current_matches {
            matched_ref[star_match.reference] = true;
        }

        newly_matched_targets.fill(false);

        for (ref_idx, &ref_pos) in ref_stars.iter().enumerate() {
            if matched_ref[ref_idx] {
                continue;
            }

            let predicted = current_transform.apply(ref_pos);

            if let Some(nn) = target_tree.nearest_one(predicted)
                && nn.dist_sq <= threshold_sq
                && !matched_target[nn.index]
                && !newly_matched_targets[nn.index]
            {
                current_matches.push(StarMatch {
                    reference: ref_idx,
                    target: nn.index,
                });
                newly_matched_targets[nn.index] = true;
            }
        }

        // Re-validate all matches against current transform, removing outliers
        current_matches.retain(|star_match| {
            let predicted = current_transform.apply(ref_stars[star_match.reference]);
            (predicted - target_stars[star_match.target]).length_squared() <= threshold_sq
        });

        // Stop if match count didn't change (converged)
        if current_matches.len() == prev_count {
            break;
        }

        // Refit transform with updated matches
        let all_ref: Vec<DVec2> = current_matches
            .iter()
            .map(|star_match| ref_stars[star_match.reference])
            .collect();
        let all_target: Vec<DVec2> = current_matches
            .iter()
            .map(|star_match| target_stars[star_match.target])
            .collect();

        match estimate_transform(&all_ref, &all_target, transform_type) {
            Some(new_transform) => current_transform = new_transform,
            None => break,
        }
    }

    // Ensure we never return fewer matches than we started with
    if current_matches.len() < inlier_matches.len() {
        return RecoveredMatches {
            transform: *transform,
            matches: inlier_matches.to_vec(),
        };
    }

    RecoveredMatches {
        transform: current_transform,
        matches: current_matches,
    }
}

#[cfg(test)]
mod fwhm_tests {
    use crate::stacking::registration::*;

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

#[cfg(test)]
mod recovery_tests {
    use crate::stacking::registration::*;
    use crate::testing::synthetic::transforms::generate_random_positions;

    fn identity_matches(count: usize) -> Vec<StarMatch> {
        (0..count)
            .map(|index| StarMatch {
                reference: index,
                target: index,
            })
            .collect()
    }

    /// Apply a similarity transform (rotation + translation) around a center.
    fn apply_similarity(pos: DVec2, dx: f64, dy: f64, angle: f64, center: DVec2) -> DVec2 {
        let r = pos - center;
        let cos_a = angle.cos();
        let sin_a = angle.sin();
        DVec2::new(
            cos_a * r.x - sin_a * r.y + center.x + dx,
            sin_a * r.x + cos_a * r.y + center.y + dy,
        )
    }

    #[test]
    fn test_iterative_recovery_improves_on_biased_seed() {
        // Setup: 50 stars with a rotation transform. Give recover_matches
        // a slightly wrong initial transform (estimated from only 3 seed points).
        // The initial transform is close enough for some matches, but iterative
        // refinement should recover significantly more.
        let ref_stars = generate_random_positions(50, 2000.0, 2000.0, 42);

        let dx = 30.0;
        let dy = -20.0;
        let angle = 1.0_f64.to_radians();
        let center = DVec2::new(1000.0, 1000.0);

        let target_stars: Vec<DVec2> = ref_stars
            .iter()
            .map(|&p| apply_similarity(p, dx, dy, angle, center))
            .collect();

        // Create a biased initial transform from only the first 3 points
        let seed_ref: Vec<DVec2> = ref_stars[..3].to_vec();
        let seed_target: Vec<DVec2> = target_stars[..3].to_vec();
        let initial_transform =
            estimate_transform(&seed_ref, &seed_target, TransformType::Euclidean).unwrap();

        let seed_matches = identity_matches(3);
        let threshold = 3.0; // ~3px

        let RecoveredMatches {
            transform: refined_transform,
            matches: recovered_matches,
        } = recover_matches(
            &ref_stars,
            &target_stars,
            &initial_transform,
            &seed_matches,
            threshold,
            TransformType::Euclidean,
        );

        // Iterative recovery should find many more matches than the 3 seeds
        assert!(
            recovered_matches.len() > 10,
            "Expected significant recovery, got only {} matches from 3 seeds",
            recovered_matches.len()
        );

        // Verify the refined transform is accurate
        let mut max_error = 0.0f64;
        for star_match in &recovered_matches {
            let predicted = refined_transform.apply(ref_stars[star_match.reference]);
            let error = (predicted - target_stars[star_match.target]).length();
            max_error = max_error.max(error);
        }
        assert!(
            max_error < threshold,
            "All recovered matches should be within threshold, max_error={}",
            max_error
        );
    }

    #[test]
    fn test_iterative_recovery_converges() {
        // With a perfect initial transform, recovery should converge in 1 pass
        // (no improvement possible after first pass finds all matches).
        let ref_stars = generate_random_positions(30, 1000.0, 1000.0, 99);

        let dx = 15.0;
        let dy = -10.0;
        let target_stars: Vec<DVec2> = ref_stars.iter().map(|&p| p + DVec2::new(dx, dy)).collect();

        let transform =
            estimate_transform(&ref_stars, &target_stars, TransformType::Translation).unwrap();

        // Start with only 5 seed matches
        let seed_matches = identity_matches(5);

        let RecoveredMatches {
            matches: recovered, ..
        } = recover_matches(
            &ref_stars,
            &target_stars,
            &transform,
            &seed_matches,
            3.0,
            TransformType::Translation,
        );

        // With a perfect transform, should recover all 30 matches
        assert_eq!(
            recovered.len(),
            30,
            "Perfect transform should recover all stars, got {}",
            recovered.len()
        );
    }

    #[test]
    fn test_iterative_recovery_never_loses_matches() {
        // Ensure the safety fallback works: we never return fewer matches
        // than we started with.
        let ref_stars = generate_random_positions(20, 1000.0, 1000.0, 77);
        let target_stars: Vec<DVec2> = ref_stars
            .iter()
            .map(|&p| p + DVec2::new(10.0, 5.0))
            .collect();

        let transform =
            estimate_transform(&ref_stars, &target_stars, TransformType::Translation).unwrap();

        let seed_matches = identity_matches(10);

        let RecoveredMatches {
            matches: recovered, ..
        } = recover_matches(
            &ref_stars,
            &target_stars,
            &transform,
            &seed_matches,
            3.0,
            TransformType::Translation,
        );

        assert!(
            recovered.len() >= seed_matches.len(),
            "Should never lose matches: started with {}, got {}",
            seed_matches.len(),
            recovered.len()
        );
    }

    #[test]
    fn test_iterative_recovery_removes_outliers() {
        // Start with some incorrect seed matches. The re-validation step
        // should remove them during iteration.
        let ref_stars = generate_random_positions(30, 1000.0, 1000.0, 55);
        let target_stars: Vec<DVec2> = ref_stars
            .iter()
            .map(|&p| p + DVec2::new(20.0, -15.0))
            .collect();

        let transform =
            estimate_transform(&ref_stars, &target_stars, TransformType::Translation).unwrap();

        // Good matches plus 2 deliberately wrong matches
        let mut seed_matches = identity_matches(8);
        // Wrong: ref[8] matched to target[15], ref[9] matched to target[20]
        seed_matches.push(StarMatch {
            reference: 8,
            target: 15,
        });
        seed_matches.push(StarMatch {
            reference: 9,
            target: 20,
        });

        let RecoveredMatches {
            matches: recovered, ..
        } = recover_matches(
            &ref_stars,
            &target_stars,
            &transform,
            &seed_matches,
            3.0,
            TransformType::Translation,
        );

        // Wrong matches should be removed, correct ones kept
        for star_match in &recovered {
            assert_eq!(
                star_match.reference, star_match.target,
                "All recovered matches should be correct correspondences (r==t for this synthetic data)"
            );
        }

        // Should still recover many correct matches
        assert!(
            recovered.len() >= 20,
            "Should recover many correct matches after removing outliers, got {}",
            recovered.len()
        );
    }
}
