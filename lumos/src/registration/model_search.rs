//! [`ModelSearch`]: one frame pair's matches, fitted by the model a registration asks for.

use std::time::Instant;

use crate::registration::distortion::sip::SipModel;
use crate::registration::error::{FailedModel, RegistrationError};
use crate::registration::final_fit::fit_catalogs::FitCatalogs;
use crate::registration::final_fit::{FinalFit, FinalFitFailure, FitModel};
use crate::registration::point_pairs::PointPairs;
use crate::registration::ransac::RansacEstimator;
use crate::registration::registration_config::RegistrationConfig;
use crate::registration::transform::{TransformModel, TransformType};
use crate::registration::triangle::voting::MatchIndices;
use crate::registration::tuning;

/// The models `Auto` chooses among, from the fewest degrees of freedom to the most.
const AUTO_MODELS: [TransformType; 4] = [
    TransformType::Euclidean,
    TransformType::Similarity,
    TransformType::Affine,
    TransformType::Homography,
];

/// What every model a registration tries is fitted from: RANSAC's matched positions and their
/// confidences, and the full catalogs the final fit reads.
#[derive(Debug)]
pub(super) struct ModelSearch<'a> {
    /// The triangle matches' positions, among each catalog's brightest stars.
    pub(super) pairs: PointPairs,
    /// Each match's confidence, by index into `pairs`.
    pub(super) confidences: Vec<f64>,
    pub(super) catalogs: &'a FitCatalogs,
    /// The noise scale RANSAC scores at.
    pub(super) max_sigma: f64,
    pub(super) config: &'a RegistrationConfig,
}

impl ModelSearch<'_> {
    /// The fit of `model`, or of the model `Auto` chooses, with `sip` when one is asked for.
    pub(super) fn fit(
        &self,
        model: TransformModel,
        sip: Option<SipModel>,
    ) -> Result<FinalFit, RegistrationError> {
        match model {
            TransformModel::Auto => self.select_by_gric(sip),
            TransformModel::Fixed(transform) => {
                self.estimate_and_refine(FitModel { transform, sip })
            }
        }
    }

    /// `Auto` model selection by the geometric robust information criterion (Torr 1998): every
    /// model of [`AUTO_MODELS`] is fitted — up to an affine map with a SIP correction — and the
    /// one of lowest
    ///
    /// `GRIC = Σ min(zᵢ²/ŝ², λ₃·(r − d)) + λ₂·k`
    ///
    /// wins, ties to the simpler, among the models whose RMS meets the caller's `max_rms_error` —
    /// a model the caller will refuse cannot be the answer while another passes — or among all
    /// when none does, for `register` to refuse. The sum runs over the union of every fitted
    /// model's pairs, each model's residual on each in units of the pair's σ, so a pair a model
    /// cannot follow costs it the cap rather than leaving its RMS unmeasured; `ŝ` is the most
    /// general fitted model's robust scale, the noise estimate all are judged by. A pair of 2-D
    /// points is `r = 4` numbers and every candidate a 2-D map, `d = 2`, so the cap is
    /// `λ₃·(r − d)` = 4 at Torr's `λ₃ = 2`, Torr's `λ₁·d·n` is the same for every candidate and
    /// drops out, and each parameter costs `λ₂ = ln(r·n)`, with `k` the transform's parameters
    /// and the SIP correction's.
    ///
    /// Every model's failure reaches the caller, as
    /// [`EveryModelFailed`](RegistrationError::EveryModelFailed), when none of them fit at all.
    /// The models fail independently — RANSAC estimates the model it was given — so no single
    /// error stands in for the others.
    fn select_by_gric(&self, sip: Option<SipModel>) -> Result<FinalFit, RegistrationError> {
        let mut fitted: Vec<(FitModel, FinalFit)> = Vec::new();
        let mut failures: Vec<FailedModel> = Vec::new();
        // A homography has no SIP fit: its perspective terms are the correction's quadratic ones.
        let candidates = AUTO_MODELS
            .into_iter()
            .filter(|&transform| sip.is_none() || transform != TransformType::Homography);
        for transform in candidates {
            let model = FitModel { transform, sip };
            match self.estimate_and_refine(model) {
                Ok(fit) => fitted.push((model, fit)),
                // An invalid config fails identically on every model and is the run's own fault
                // rather than the pair's — `AlignStackConfig::align_stack_images` keys a whole-run
                // abort on that distinction, so it must not be buried in a per-model report.
                // `register` validates first, so this guards the ordering rather than a reachable
                // path.
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
                    (self.catalogs.normalized_residual(&fit.warp, pair) / scale)
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
        let max_rms_error = self.config.max_rms_error;
        let any_accurate = fitted.iter().any(|(_, fit)| rms(fit) <= max_rms_error);
        let scores: Vec<f64> = fitted
            .iter()
            .map(|candidate| {
                if any_accurate && rms(&candidate.1) > max_rms_error {
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

    /// RANSAC's estimate of `model`'s transform, then the final fit of `model` over the full
    /// catalogs.
    pub(super) fn estimate_and_refine(
        &self,
        model: FitModel,
    ) -> Result<FinalFit, RegistrationError> {
        let t0 = Instant::now();
        let ransac = RansacEstimator::new(self.config.ransac.clone(), self.max_sigma);
        let ransac_result = ransac
            .estimate(&self.pairs, &self.confidences, model.transform)
            .map_err(|failure| RegistrationError::RansacFailed {
                reason: failure.reason,
                iterations: failure.iterations,
                best_inlier_count: failure.best_inlier_count,
            })?;
        let ransac_ms = t0.elapsed().as_secs_f64() * 1000.0;

        let t0 = Instant::now();
        let fit = self
            .catalogs
            .fit(
                ransac_result.transform,
                model,
                tuning::recovery_radius(self.max_sigma),
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
        // The floor the matcher was held to holds for the fit too: a transform supported by a
        // minimal sample has a near-zero RMS by construction, so the accuracy gate alone would
        // pass it.
        let min_matches = self.config.matching.min_matches;
        if fit.matches.len() < min_matches {
            return Err(RegistrationError::TooFewInliers {
                found: fit.matches.len(),
                required: min_matches,
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
}
