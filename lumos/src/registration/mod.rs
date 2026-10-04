//! Image registration module for astronomical image alignment.
//!
//! This module provides star-based image registration using triangle matching
//! and RANSAC for robust transformation estimation.
//!
//! # Quick Start
//!
//! ```no_run
//! use lumos::detection::Star;
//! use lumos::{LinearImage, RegistrationConfig, register, warp};
//!
//! # fn example(ref_stars: &[Star], target_stars: &[Star], target_image: &LinearImage)
//! # -> Result<(), lumos::RegistrationError> {
//! let config = RegistrationConfig::default();
//!
//! // Register stars from two images
//! let result = register(ref_stars, target_stars, &config)?;
//! println!(
//!     "Matched {} stars, RMS = {:.2}px",
//!     result.num_inliers(),
//!     result.rms_error()
//! );
//!
//! // Reproject the target onto the reference's grid, into a new image
//! let aligned = warp(target_image, &result.warp_transform(), config.warp);
//! # Ok(())
//! # }
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
//! | Auto | - | Every model from Euclidean to Homography fitted, the lowest GRIC (Torr 1998) wins |
//!
//! # Configuration Presets
//!
//! - [`RegistrationConfig::default()`] — Balanced settings for most astrophotography
//! - [`RegistrationConfig::fast()`] — Fewer iterations, bilinear interpolation
//! - [`RegistrationConfig::precise()`] — More iterations, SIP distortion correction
//! - [`RegistrationConfig::wide_field()`] — Homography + SIP for wide-field lenses
//! - [`RegistrationConfig::mosaic()`] — Allows larger rotations and scale differences

pub(crate) mod distortion;
mod final_fit;
mod point_normalization;
mod point_pairs;
pub(crate) mod ransac;
pub(crate) mod registration_config;
pub(crate) mod resample;
pub(crate) mod result;
mod spatial;
pub(crate) mod transform;
pub(crate) mod triangle;
mod tuning;

use std::time::Instant;

use glam::DVec2;

use crate::math::statistics::median_mut;
use crate::registration::final_fit::{FinalFit, FinalFitFailure, FitCatalogs, FitModel, SipModel};
use crate::registration::ransac::RansacEstimator;
use crate::registration::registration_config::RegistrationConfig;
use crate::registration::result::{
    FailedModel, RegistrationCatalog, RegistrationError, RegistrationResult,
};
use crate::registration::spatial::KdTree;
use crate::registration::transform::{TransformModel, TransformType};
use crate::registration::triangle::matching;
use crate::registration::triangle::voting::{MatchIndices, PointMatch};
use crate::star_detection::star::Star;

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
/// [`RegistrationError::InvalidConfig`] if `config` fails validation (see [`RegistrationConfig::validate`]),
/// and the matching/accuracy failures below. A caller that runs this per frame pair must treat
/// `InvalidConfig` apart from the rest: every other variant describes one pair, and is a frame to
/// drop, but an invalid config fails every pair identically and is the run's own fault.
///
/// # Example
///
/// ```no_run
/// use lumos::detection::Star;
/// use lumos::{RegistrationConfig, TransformModel, TransformType, register};
///
/// # fn example(ref_stars: &[Star], target_stars: &[Star]) -> Result<(), lumos::RegistrationError> {
/// // With defaults
/// let result = register(ref_stars, target_stars, &RegistrationConfig::default())?;
///
/// // With custom config
/// let config = RegistrationConfig {
///     transform_type: TransformModel::Fixed(TransformType::Similarity),
///     ..RegistrationConfig::default()
/// };
/// let result = register(ref_stars, target_stars, &config)?;
///
/// println!("Matched {} stars", result.num_inliers());
/// println!("RMS error: {:.2} pixels", result.rms_error());
/// # Ok(())
/// # }
/// ```
pub fn register(
    ref_stars: &[Star],
    target_stars: &[Star],
    config: &RegistrationConfig,
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
    let catalogs =
        FitCatalogs::new(ref_stars, target_stars).ok_or(RegistrationError::InsufficientStars {
            found: 0,
            required: required_stars,
        })?;
    let sip = config.sip.as_ref().map(|sip| SipModel {
        order: sip.order,
        origin: sip
            .reference_point
            .unwrap_or_else(|| bounding_box_centre(ref_stars)),
    });

    // The brightest `max_stars` of each set, as trees: triangle matching forms its triangles over
    // both, and RANSAC samples their matches for every model.
    let brightest = |stars: &[Star]| {
        KdTree::build(
            stars
                .iter()
                .take(config.matching.max_stars)
                .map(|s| s.pos)
                .collect(),
        )
        .expect("the star gates leave at least three stars in each set")
    };
    let ref_tree = brightest(ref_stars);
    let target_tree = brightest(target_stars);

    // Triangle matching
    let t0 = Instant::now();
    let matches = matching::match_triangles(
        &ref_tree,
        &target_tree,
        &config.matching.triangle,
        max_sigma,
    );
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
    let fit = match config.transform_type {
        TransformModel::Auto => select_by_gric(
            ref_tree.points(),
            &target_tree,
            &matches,
            &catalogs,
            sip,
            max_sigma,
            config,
        ),
        TransformModel::Fixed(transform_type) => estimate_and_refine(
            ref_tree.points(),
            &target_tree,
            &matches,
            &catalogs,
            FitModel {
                transform: transform_type,
                sip,
            },
            max_sigma,
            config,
        ),
    }?;

    let result = RegistrationResult::new(fit.warp.transform, fit.warp.sip, fit.matches)
        .with_elapsed(start.elapsed().as_secs_f64() * 1000.0);
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
        let fields = [
            ("FWHM", f64::from(star.fwhm)),
            ("flux", f64::from(star.flux)),
            ("SNR", f64::from(star.snr)),
            ("peak", f64::from(star.peak)),
            ("sharpness", f64::from(star.sharpness)),
            ("eccentricity", f64::from(star.eccentricity)),
            ("GROUND", f64::from(star.roundness.ground)),
            ("SROUND", f64::from(star.roundness.sround)),
            ("position σ", star.position_sigma),
        ];
        if let Some(&(field, value)) = fields.iter().find(|(_, value)| !value.is_finite()) {
            return Err(RegistrationError::InvalidStarField {
                catalog,
                index,
                field,
                value,
            });
        }
    }
    Ok(())
}

/// The centre of the stars' bounding box: the SIP origin when none is configured, shared by every
/// frame registered to the same reference.
fn bounding_box_centre(stars: &[Star]) -> DVec2 {
    let (low, high) = stars.iter().fold(
        (DVec2::splat(f64::INFINITY), DVec2::splat(f64::NEG_INFINITY)),
        |(low, high), star| (low.min(star.pos), high.max(star.pos)),
    );
    (low + high) / 2.0
}

/// Compute the median FWHM from two sets of stars.
fn median_fwhm(ref_stars: &[Star], target_stars: &[Star]) -> f64 {
    let mut fwhms: Vec<f32> = ref_stars
        .iter()
        .chain(target_stars.iter())
        .map(|s| s.fwhm)
        .collect();

    f64::from(median_mut(&mut fwhms))
}

/// The models `Auto` chooses among, from the fewest degrees of freedom to the most.
const AUTO_MODELS: [TransformType; 4] = [
    TransformType::Euclidean,
    TransformType::Similarity,
    TransformType::Affine,
    TransformType::Homography,
];

/// `Auto` model selection by the geometric robust information criterion (Torr 1998): every model
/// of [`AUTO_MODELS`] is fitted — up to an affine map with a SIP correction — and the one of lowest
///
/// `GRIC = Σ min(zᵢ²/ŝ², λ₃·(r − d)) + λ₂·k`
///
/// wins, ties to the simpler, among the models whose RMS meets the caller's `max_rms_error` — a
/// model the caller will refuse cannot be the answer while another passes — or among all when none
/// does, for `register` to refuse. The sum runs over the union of every fitted model's pairs, each
/// model's residual on each in units of the pair's σ, so a pair a model cannot follow costs it the
/// cap rather than leaving its RMS unmeasured; `ŝ` is the most general fitted model's robust
/// scale, the noise estimate all are judged by. A pair of 2-D points is `r = 4` numbers and every
/// candidate a 2-D map, `d = 2`, so the cap is `λ₃·(r − d)` = 4 at Torr's `λ₃ = 2`, Torr's
/// `λ₁·d·n` is the same for every candidate and drops out, and each parameter costs
/// `λ₂ = ln(r·n)`, with `k` the transform's parameters and the SIP correction's.
///
/// Every model's failure reaches the caller, as
/// [`EveryModelFailed`](RegistrationError::EveryModelFailed), when none of them fit at all.
/// The models fail independently — RANSAC estimates the model it was given — so no single error
/// stands in for the others.
fn select_by_gric(
    ref_positions: &[DVec2],
    target_tree: &KdTree,
    matches: &[PointMatch],
    catalogs: &FitCatalogs,
    sip: Option<SipModel>,
    max_sigma: f64,
    config: &RegistrationConfig,
) -> Result<FinalFit, RegistrationError> {
    let mut fitted: Vec<(FitModel, FinalFit)> = Vec::new();
    let mut failures: Vec<FailedModel> = Vec::new();
    // A homography has no SIP fit: its perspective terms are the correction's quadratic ones.
    let candidates = AUTO_MODELS
        .into_iter()
        .filter(|&transform| sip.is_none() || transform != TransformType::Homography);
    for transform in candidates {
        let model = FitModel { transform, sip };
        match estimate_and_refine(
            ref_positions,
            target_tree,
            matches,
            catalogs,
            model,
            max_sigma,
            config,
        ) {
            Ok(fit) => fitted.push((model, fit)),
            // An invalid config fails identically on every model and is the run's own fault rather
            // than the pair's — `align_and_stack` keys a whole-run abort on that distinction, so it
            // must not be buried in a per-model report. `register` validates first, so this
            // guards the ordering rather than a reachable path.
            Err(error @ RegistrationError::InvalidConfig(_)) => return Err(error),
            Err(error) => {
                tracing::debug!(model = ?transform, %error, "Auto candidate failed");
                failures.push(FailedModel {
                    model: transform,
                    error: Box::new(error),
                });
            }
        }
    }
    let Some(scale) = fitted.last().map(|(_, fit)| fit.scale) else {
        return Err(RegistrationError::EveryModelFailed { failures });
    };
    let mut union: Vec<MatchIndices> = fitted
        .iter()
        .flat_map(|(_, fit)| fit.matches.iter().map(|star_match| star_match.indices))
        .collect();
    union.sort_unstable_by_key(|pair| (pair.reference, pair.target));
    union.dedup();
    let per_parameter = (4.0 * union.len() as f64).ln();
    let gric = |(model, fit): &(FitModel, FinalFit)| {
        union
            .iter()
            .map(|&pair| {
                (catalogs.normalized_residual(&fit.warp, pair) / scale)
                    .powi(2)
                    .min(4.0)
            })
            .sum::<f64>()
            + per_parameter * model.parameter_count() as f64
    };
    let rms = |fit: &FinalFit| {
        (fit.matches
            .iter()
            .map(|star_match| star_match.residual * star_match.residual)
            .sum::<f64>()
            / fit.matches.len() as f64)
            .sqrt()
    };
    let any_accurate = fitted
        .iter()
        .any(|(_, fit)| rms(fit) <= config.max_rms_error);
    let scores: Vec<f64> = fitted
        .iter()
        .map(|candidate| {
            if any_accurate && rms(&candidate.1) > config.max_rms_error {
                f64::INFINITY
            } else {
                gric(candidate)
            }
        })
        .collect();
    let best = scores.iter().enumerate().fold(
        0,
        |best, (i, &score)| if score < scores[best] { i } else { best },
    );
    tracing::debug!(?scores, chosen = ?fitted[best].0.transform, "Auto chose by GRIC");
    Ok(fitted.swap_remove(best).1)
}

/// Run RANSAC estimation for `model`'s transform, then the final fit of `model` over the full
/// catalogs.
fn estimate_and_refine(
    ref_stars: &[DVec2],
    target_tree: &KdTree,
    matches: &[PointMatch],
    catalogs: &FitCatalogs,
    model: FitModel,
    max_sigma: f64,
    config: &RegistrationConfig,
) -> Result<FinalFit, RegistrationError> {
    let target_stars = target_tree.points();
    let t0 = Instant::now();
    let ransac = RansacEstimator::new(config.ransac.clone(), max_sigma);
    let ransac_result = ransac
        .estimate(matches, ref_stars, target_stars, model.transform)
        .map_err(|failure| RegistrationError::RansacFailed {
            reason: failure.reason,
            iterations: failure.iterations,
            best_inlier_count: failure.best_inlier_count,
        })?;
    let ransac_ms = t0.elapsed().as_secs_f64() * 1000.0;

    let t0 = Instant::now();
    let fit = FinalFit::run(
        catalogs,
        ransac_result.transform,
        model,
        tuning::recovery_radius(max_sigma),
    )
    .map_err(|failure| match failure {
        FinalFitFailure::TooFewPairs { found, required } => {
            RegistrationError::TooFewInliers { found, required }
        }
        FinalFitFailure::TooFewSipPairs { found, required } => {
            RegistrationError::InsufficientSipPoints { found, required }
        }
        FinalFitFailure::Degenerate => RegistrationError::DegenerateFit {
            model: model.transform,
        },
    })?;
    let fit_ms = t0.elapsed().as_secs_f64() * 1000.0;
    // The floor the matcher was held to holds for the fit too: a transform supported by a minimal
    // sample has a near-zero RMS by construction, so the accuracy gate alone would pass it.
    if fit.matches.len() < config.matching.min_matches {
        return Err(RegistrationError::TooFewInliers {
            found: fit.matches.len(),
            required: config.matching.min_matches,
        });
    }
    tracing::debug!(
        ransac_ms,
        fit_ms,
        ransac_inliers = ransac_result.inliers.len(),
        fit_scale = fit.scale,
        "Registration sub-step timing"
    );
    Ok(fit)
}

#[cfg(all(test, feature = "bench"))]
mod bench;
#[cfg(test)]
mod tests;
