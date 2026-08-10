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
//! println!(
//!     "Matched {} stars, RMS = {:.2}px",
//!     result.num_inliers(),
//!     result.rms_error()
//! );
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
pub(crate) mod ransac;
pub(crate) mod resample;
pub(crate) mod result;
mod spatial;
pub(crate) mod transform;
pub(crate) mod triangle;
mod tuning;

#[cfg(all(test, feature = "internals"))]
mod bench;
#[cfg(all(test, feature = "real-data"))]
mod real_data_tests;
#[cfg(test)]
mod tests;

use crate::stacking::registration::triangle::voting::MatchIndices;
use config::Config;
use distortion::sip::SipPolynomial;
use result::{
    RansacFailureReason, RegistrationCatalog, RegistrationError, RegistrationResult, StarMatch,
};
use transform::{Transform, TransformModel, TransformType};

use std::time::Instant;

use glam::DVec2;

use crate::math::statistics::median_f32_mut;
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
/// # Errors
///
/// [`RegistrationError::InvalidConfig`] if `config` fails validation (see [`Config::validate`]),
/// and the matching/accuracy failures below. A caller that runs this per frame pair must treat
/// `InvalidConfig` apart from the rest: every other variant describes one pair, and is a frame to
/// drop, but an invalid config fails every pair identically and is the run's own fault.
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
///     transform_type: TransformModel::Fixed(TransformType::Similarity),
///     ..Config::default()
/// };
/// let result = register(&ref_stars, &target_stars, &config)?;
///
/// println!("Matched {} stars", result.num_inliers());
/// println!("RMS error: {:.2} pixels", result.rms_error());
/// ```
pub fn register(
    ref_stars: &[Star],
    target_stars: &[Star],
    config: &Config,
) -> Result<RegistrationResult, RegistrationError> {
    config.validate()?;
    validate_catalog(ref_stars, RegistrationCatalog::Reference)?;
    validate_catalog(target_stars, RegistrationCatalog::Target)?;
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
    let max_sigma = tuning::max_sigma_from_fwhm(median_fwhm(ref_stars, target_stars));

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
    let result = match config.transform_type {
        TransformModel::Auto => auto_ladder(
            &ref_positions,
            &target_positions,
            &matches,
            max_sigma,
            config,
        ),
        TransformModel::Fixed(transform_type) => estimate_and_refine(
            &ref_positions,
            &target_positions,
            &matches,
            transform_type,
            max_sigma,
            config,
        ),
    }?;

    let result = result.with_elapsed(start.elapsed().as_secs_f64() * 1000.0);
    let rms_error = result.rms_error();

    if rms_error > config.max_rms_error {
        return Err(RegistrationError::AccuracyTooLow {
            rms_error,
            max_allowed: config.max_rms_error,
        });
    }

    Ok(result)
}

fn validate_catalog(stars: &[Star], catalog: RegistrationCatalog) -> Result<(), RegistrationError> {
    for (index, star) in stars.iter().enumerate() {
        if !star.pos.x.is_finite() || !star.pos.y.is_finite() {
            return Err(RegistrationError::InvalidStarPosition {
                catalog,
                index,
                position: star.pos,
            });
        }
        if !star.fwhm.is_finite() {
            return Err(RegistrationError::InvalidStarFwhm {
                catalog,
                index,
                value: star.fwhm,
            });
        }
    }
    Ok(())
}

/// Compute the median FWHM from two sets of stars.
fn median_fwhm(ref_stars: &[Star], target_stars: &[Star]) -> f64 {
    let mut fwhms: Vec<f32> = ref_stars
        .iter()
        .chain(target_stars.iter())
        .map(|s| s.fwhm)
        .collect();

    median_f32_mut(&mut fwhms) as f64
}

/// `Auto` model selection: estimate transforms from fewest to most degrees of freedom and accept
/// the first whose RMS clears [`tuning::AUTO_UPGRADE_THRESHOLD`] — the *simplest model that fits*,
/// so the alignment isn't overfit to star-centroid noise (every extra DOF soaks up noise and
/// generalizes worse). Falls through to the most general model (Homography) when no simpler rung
/// clears the bar; the caller's `max_rms_error` gate then has the final say on that result.
///
/// The bar is the *stricter* of the ladder's own threshold and the caller's `max_rms_error`. Taking
/// only the ladder's would let a tight `max_rms_error` fail the whole registration on a rung this
/// function accepted, while a later rung would have satisfied it.
///
/// The ladder is Euclidean → Similarity → Affine → Homography (rigid → +scale → +shear →
/// projective); earlier this only tried Similarity then jumped straight to Homography, so same-scale
/// rigid sets were fit with a needless scale DOF and mild differential distortion overshot to the
/// full projective model.
///
/// A rung that fails outright is logged and the ladder continues, but only Homography's error can
/// reach the caller: it is the most general model, so its failure is the one that describes the
/// data rather than the model. The earlier rungs' errors would otherwise be lost entirely, and the
/// answer to "why did `Auto` land on Homography?" is in that log.
fn auto_ladder(
    ref_positions: &[DVec2],
    target_positions: &[DVec2],
    matches: &[PointMatch],
    max_sigma: f64,
    config: &Config,
) -> Result<RegistrationResult, RegistrationError> {
    let bar = tuning::AUTO_UPGRADE_THRESHOLD.min(config.max_rms_error);
    for model in [
        TransformType::Euclidean,
        TransformType::Similarity,
        TransformType::Affine,
    ] {
        match estimate_and_refine(
            ref_positions,
            target_positions,
            matches,
            model,
            max_sigma,
            config,
        ) {
            Ok(result) if result.rms_error() <= bar => return Ok(result),
            Ok(result) => tracing::debug!(
                ?model,
                rms_error = result.rms_error(),
                bar,
                "Auto rung fit but missed the bar"
            ),
            Err(error) => tracing::debug!(?model, %error, "Auto rung failed"),
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
        .map(|&i| matches[i].indices())
        .collect();

    let t0 = Instant::now();
    let RecoveredMatches {
        transform,
        matches: inlier_matches,
    } = recover_matches(
        ref_stars,
        target_stars,
        &ransac_result.transform,
        &inlier_matches,
        tuning::recovery_radius(max_sigma),
        transform_type,
    );
    let recovery_ms = t0.elapsed().as_secs_f64() * 1000.0;

    let t0 = Instant::now();
    let sip_fit = if let Some(sip_config) = &config.sip {
        // Materialized rather than indexed through `inlier_matches`: the fitter takes paired
        // position slices and knows nothing about match indices, which is the layering that keeps
        // `distortion` independent of how the matches were found. Only the SIP path pays for it,
        // and it pays once — `unzip` fills both from a single walk.
        let (inlier_ref, inlier_target): (Vec<DVec2>, Vec<DVec2>) = inlier_matches
            .iter()
            .map(|star_match| {
                (
                    ref_stars[star_match.reference],
                    target_stars[star_match.target],
                )
            })
            .unzip();

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

    let matched_stars: Vec<StarMatch> = inlier_matches
        .iter()
        .map(|indices| {
            let ref_pos = ref_stars[indices.reference];
            let target_pos = target_stars[indices.target];
            let corrected_r = match sip_polynomial {
                Some(sip) => sip.correct(ref_pos),
                None => ref_pos,
            };
            let p = transform.apply(corrected_r);
            StarMatch::measured(*indices, (p - target_pos).length())
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

    Ok(RegistrationResult::new(transform, sip_fit, matched_stars))
}

/// Maximum iterations for iterative match recovery.
/// Convergence is typically reached in 2-3 passes; diminishing returns after that.
const RECOVERY_MAX_ITERATIONS: usize = 5;

#[derive(Debug)]
struct RecoveredMatches {
    transform: Transform,
    matches: Vec<MatchIndices>,
}

fn recover_matches(
    ref_stars: &[DVec2],
    target_stars: &[DVec2],
    transform: &Transform,
    inlier_matches: &[MatchIndices],
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
    // Refit inputs, rebuilt per pass from the pass's own matches; only the capacity carries over.
    let (mut all_ref, mut all_target) = (Vec::new(), Vec::new());

    for _ in 0..RECOVERY_MAX_ITERATIONS {
        let prev_count = current_matches.len();

        matched_target.fill(false);
        matched_ref.fill(false);
        for star_match in &current_matches {
            matched_target[star_match.target] = true;
            matched_ref[star_match.reference] = true;
        }

        for (ref_idx, &ref_pos) in ref_stars.iter().enumerate() {
            if matched_ref[ref_idx] {
                continue;
            }

            let predicted = current_transform.apply(ref_pos);

            // Claiming the target in `matched_target` is what stops a second reference star from
            // taking it later in this same pass — no separate "newly matched" bitmap needed.
            if let Some(nn) = target_tree.nearest_one(predicted)
                && nn.dist_sq <= threshold_sq
                && !matched_target[nn.index]
            {
                current_matches.push(MatchIndices {
                    reference: ref_idx,
                    target: nn.index,
                });
                matched_target[nn.index] = true;
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
        all_ref.clear();
        all_target.clear();
        all_ref.reserve_exact(current_matches.len());
        all_target.reserve_exact(current_matches.len());
        for star_match in &current_matches {
            all_ref.push(ref_stars[star_match.reference]);
            all_target.push(target_stars[star_match.target]);
        }

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
