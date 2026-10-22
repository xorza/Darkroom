//! [`ModelSearch`]: one frame pair's matches, fitted by the model a registration asks for.

use std::time::Instant;

use arrayvec::ArrayVec;
use glam::DVec2;

use crate::registration::distortion::sip::{SipConfig, SipModel};
use crate::registration::error::{FailedModel, RegistrationError};
use crate::registration::final_fit::fit_catalogs::FitCatalogs;
use crate::registration::final_fit::{FinalFit, FinalFitFailure, FitModel};
use crate::registration::match_idxs::MatchIdxs;
use crate::registration::point_pairs::PointPairs;
use crate::registration::ransac::{RansacEstimator, RansacResult};
use crate::registration::transform::TransformType;
use crate::registration::transform::linear_model::LinearModel;
use crate::registration::transform_model::TransformModel;

/// The models `Auto` chooses among with no SIP correction, from the fewest degrees of freedom to
/// the most; under one, [`LinearModel::AUTO`].
const AUTO_MODELS: [TransformType; 4] = [
    TransformType::Euclidean,
    TransformType::Similarity,
    TransformType::Affine,
    TransformType::Homography,
];

/// What every model a registration tries is fitted from — RANSAC's matched positions and their
/// confidences, and the full catalogs the final fit reads — by what: the RANSAC estimator, the
/// radius the final fit recovers pairs within, and the gates every fit is held to.
#[derive(Debug)]
pub(super) struct ModelSearch<'a> {
    /// The triangle matches' positions, among each catalog's brightest stars.
    pub(super) pairs: PointPairs,
    /// Each match's confidence, by index into `pairs`.
    pub(super) confidences: Vec<f64>,
    pub(super) catalogs: &'a FitCatalogs,
    pub(super) ransac: RansacEstimator,
    pub(super) recovery_radius: f64,
    /// The matcher's floor of pairs, which a fit is held to as well.
    pub(super) min_matches: usize,
    /// The caller's RMS gate, which `Auto` prefers a model inside of.
    pub(super) max_rms_error: f64,
}

impl ModelSearch<'_> {
    /// The fit of `model`, or of the model `Auto` chooses, its SIP correction taken about its own
    /// reference point or else `origin`.
    pub(super) fn fit(
        &self,
        model: TransformModel,
        origin: DVec2,
    ) -> Result<FinalFit, RegistrationError> {
        let sip = |config: SipConfig| SipModel {
            order: config.order,
            origin: config.reference_point.unwrap_or(origin),
        };
        match model {
            TransformModel::Auto { sip: config } => self.select_by_gric(config.map(sip)),
            TransformModel::Fixed(transform) => {
                self.estimate_and_refine(FitModel::Plain(transform))
            }
            TransformModel::FixedWithSip(linear, config) => {
                self.estimate_and_refine(FitModel::WithSip(linear, sip(config)))
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
        let candidates: ArrayVec<FitModel, 4> = match sip {
            None => AUTO_MODELS.into_iter().map(FitModel::Plain).collect(),
            Some(sip) => LinearModel::AUTO
                .into_iter()
                .map(|linear| FitModel::WithSip(linear, sip))
                .collect(),
        };
        for model in candidates {
            match self.estimate_and_refine(model) {
                Ok(fit) => fitted.push((model, fit)),
                Err(error) => {
                    tracing::debug!(model = ?model.transform_type(), %error, "Auto candidate failed");
                    failures.push(FailedModel {
                        model: model.transform_type(),
                        error: Box::new(error),
                    });
                }
            }
        }
        let Some(scale) = fitted.last().map(|(_, fit)| fit.scale) else {
            return Err(RegistrationError::EveryModelFailed { failures });
        };
        let mut union: Vec<MatchIdxs> = fitted
            .iter()
            .flat_map(|(_, fit)| fit.matches.iter().map(|star_match| star_match.idxs))
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
        let max_rms_error = self.max_rms_error;
        let any_accurate = fitted.iter().any(|(_, fit)| fit.rms_error <= max_rms_error);
        let scores: Vec<f64> = fitted
            .iter()
            .map(|candidate| {
                if any_accurate && candidate.1.rms_error > max_rms_error {
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
        tracing::debug!(?scores, chosen = ?fitted[best].0.transform_type(), "Auto chose by GRIC");
        Ok(fitted.swap_remove(best).1)
    }

    /// RANSAC's estimate of a `transform_type` over the matches: the hypothesis the final fit
    /// starts from.
    pub(super) fn hypothesis(
        &self,
        transform_type: TransformType,
    ) -> Result<RansacResult, RegistrationError> {
        self.ransac
            .estimate(&self.pairs, &self.confidences, transform_type)
            .map_err(RegistrationError::from)
    }

    /// RANSAC's estimate of `model`'s transform, then the final fit of `model` over the full
    /// catalogs.
    pub(super) fn estimate_and_refine(
        &self,
        model: FitModel,
    ) -> Result<FinalFit, RegistrationError> {
        let t0 = Instant::now();
        let ransac_result = self.hypothesis(model.transform_type())?;
        let ransac_ms = t0.elapsed().as_secs_f64() * 1000.0;

        let t0 = Instant::now();
        let fit = self
            .catalogs
            .fit(ransac_result.transform, model, self.recovery_radius)
            .map_err(|failure| match failure {
                FinalFitFailure::TooFewPairs { found, required } => {
                    RegistrationError::TooFewInliers { found, required }
                }
                FinalFitFailure::TooFewSipPairs { found, required } => {
                    RegistrationError::TooFewSipPoints { found, required }
                }
                FinalFitFailure::Degenerate => RegistrationError::DegenerateFit {
                    model: model.transform_type(),
                },
            })?;
        let fit_ms = t0.elapsed().as_secs_f64() * 1000.0;
        // The floor the matcher was held to holds for the fit too: a transform supported by a
        // minimal sample has a near-zero RMS by construction, so the accuracy gate alone would
        // pass it.
        let min_matches = self.min_matches;
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
