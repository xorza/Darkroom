//! RANSAC (Random Sample Consensus) for robust transformation estimation.
//!
//! This module implements RANSAC with a truncated Welsch loss to robustly estimate
//! transformations in the presence of outliers: each point's residual is graded continuously at
//! the noise scale `σ_max`, rather than cut at a hand-tuned inlier threshold.
//!
//! The algorithm works by:
//! 1. Randomly sampling minimal point sets
//! 2. Computing candidate transformations
//! 3. Scoring with the truncated Welsch loss (continuous, not binary inlier/outlier)
//! 4. Keeping the best model
//! 5. Refining with least squares on inliers

pub(crate) mod ransac_config;
mod sampling;
pub(super) mod transforms;
mod welsch;

use std::mem;

use glam::DVec2;

use crate::registration::error::RansacFailureReason;
use crate::registration::point_pairs::PointPairs;
use crate::registration::ransac::ransac_config::RansacConfig;
use crate::registration::ransac::sampling::Sampler;
use crate::registration::ransac::welsch::WelschScorer;
use crate::registration::transform::{Transform, TransformType};

/// Pre-allocated buffers for local optimization (LO-RANSAC) to avoid per-iteration allocations.
#[derive(Debug)]
struct LocalOptBuffers {
    /// The inliers of the refinement that is currently best.
    inlier_buf: Vec<usize>,
    /// The inliers of the refit being scored.
    scored: Vec<usize>,
    points: PointPairs,
}

impl LocalOptBuffers {
    fn with_capacity(n: usize) -> Self {
        Self {
            inlier_buf: Vec::with_capacity(n),
            scored: Vec::with_capacity(n),
            points: PointPairs::with_capacity(n),
        }
    }
}

/// A candidate transform together with the score and inlier set it earned.
///
/// The three always move as a unit — a score belongs to the transform that produced it and to
/// the inliers it counted — so the loop swaps whole hypotheses. `inliers` is owned rather than
/// borrowed for exactly that reason: a swap trades two vector headers and keeps both
/// allocations for reuse, where separate fields would need three assignments to stay coherent.
#[derive(Debug)]
struct ScoredHypothesis {
    transform: Transform,
    score: f64,
    inliers: Vec<usize>,
}

/// Result of RANSAC estimation.
#[derive(Debug, Clone)]
pub(super) struct RansacResult {
    /// Best transformation found.
    pub(super) transform: Transform,
    /// Indices of inlier matches.
    pub(super) inliers: Vec<usize>,
}

/// Why RANSAC produced no model: the reason, how many iterations ran, and the most inliers any
/// hypothesis gathered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct RansacFailure {
    pub(super) reason: RansacFailureReason,
    pub(super) iterations: usize,
    pub(super) best_inlier_count: usize,
}

/// RANSAC estimator for robust transformation fitting.
#[derive(Debug)]
pub(super) struct RansacEstimator {
    config: RansacConfig,
    max_sigma: f64,
}

impl RansacEstimator {
    /// Create a RANSAC estimator for the runtime-derived maximum noise scale.
    pub(super) fn new(config: RansacConfig, max_sigma: f64) -> Self {
        debug_assert!(
            max_sigma.is_finite() && max_sigma > 0.0,
            "a positive, finite noise scale, not {max_sigma}"
        );
        Self { config, max_sigma }
    }

    /// Whether a hypothesis's rotation and scale lie within the configured bounds; a bound left
    /// unset passes everything.
    fn is_plausible(&self, transform: &Transform) -> bool {
        let rotation_exceeds = self
            .config
            .max_rotation
            .is_some_and(|max_rotation| transform.rotation_angle().abs() > max_rotation);
        let scale_outside = self
            .config
            .scale_range
            .is_some_and(|range| range.excludes(transform.scale_factor()));
        !rotation_exceeds && !scale_outside
    }

    /// The best `transform_type` for `pairs`, each at the `confidences` triangle matching gave it,
    /// or why there is none.
    ///
    /// The confidences guide the first hypotheses — see `sampling` — and sampling is uniform after
    /// them.
    pub(super) fn estimate(
        &self,
        pairs: &PointPairs,
        confidences: &[f64],
        transform_type: TransformType,
    ) -> Result<RansacResult, RansacFailure> {
        debug_assert_eq!(pairs.reference.len(), confidences.len());
        let min_samples = transform_type.min_points();
        if pairs.reference.len() < min_samples {
            return Err(RansacFailure {
                reason: RansacFailureReason::TooFewMatches,
                iterations: 0,
                best_inlier_count: 0,
            });
        }
        let mut sampler = Sampler::new(confidences, min_samples, &self.config);
        let guided_iterations = sampler.guided_iterations();
        Search::new(self, pairs, transform_type).run(guided_iterations, |iteration, sample| {
            sampler.draw(iteration, sample);
        })
    }
}

/// One estimation: the pairs it samples, the model it fits, and the scorer at the estimator's
/// noise scale.
#[derive(Debug)]
struct Search<'a> {
    estimator: &'a RansacEstimator,
    pairs: &'a PointPairs,
    transform_type: TransformType,
    scorer: WelschScorer,
}

impl<'a> Search<'a> {
    fn new(
        estimator: &'a RansacEstimator,
        pairs: &'a PointPairs,
        transform_type: TransformType,
    ) -> Self {
        Self {
            estimator,
            pairs,
            transform_type,
            scorer: WelschScorer::new(estimator.max_sigma),
        }
    }

    /// The RANSAC loop with truncated Welsch scoring.
    ///
    /// `sample_fn` fills the sample for each iteration, numbered from 1. The first
    /// `guided_iterations` are the sampler's guided warm-up, which the adaptive bound does not
    /// count — see `sampling`.
    fn run(
        &self,
        guided_iterations: usize,
        mut sample_fn: impl FnMut(usize, &mut Vec<usize>),
    ) -> Result<RansacResult, RansacFailure> {
        let config = &self.estimator.config;
        let max_sigma = self.estimator.max_sigma;
        let n = self.pairs.reference.len();
        let min_samples = self.transform_type.min_points();

        // `None` until a hypothesis scores at all. `scratch` is where the next hypothesis is
        // scored; a new best takes it and hands the old best's buffer back, so the loop allocates
        // nothing after its first improvement.
        let mut best: Option<ScoredHypothesis> = None;
        let mut scratch: Vec<usize> = Vec::with_capacity(n);
        let mut sample_indices: Vec<usize> = Vec::with_capacity(min_samples);
        let mut sample = PointPairs::with_capacity(min_samples);
        let mut lo_buffers = LocalOptBuffers::with_capacity(n);

        let max_iter = config.max_iterations;
        // The adaptive bound of the best hypothesis so far, in uniform iterations, checked on every
        // iteration: once that many have run, an all-inlier sample has been drawn with the
        // configured confidence, whether or not anything has improved on that hypothesis since.
        let mut iteration_bound = max_iter;
        let mut iterations = 0;

        while iterations < max_iter
            && iterations.saturating_sub(guided_iterations) < iteration_bound
        {
            iterations += 1;

            sample_fn(iterations, &mut sample_indices);
            sample.gather(&sample_indices, &self.pairs.reference, &self.pairs.target);

            if is_sample_degenerate(&sample.reference, max_sigma)
                || is_sample_degenerate(&sample.target, max_sigma)
            {
                continue;
            }

            let Some(transform) = transforms::estimate_transform(
                &sample.reference,
                &sample.target,
                self.transform_type,
            ) else {
                continue;
            };

            // Before scoring, which is the expensive part.
            if !self.estimator.is_plausible(&transform) {
                continue;
            }

            // Scored preemptively: a hypothesis that cannot beat the best stops scoring early, and
            // its `scratch` is then incomplete — which is why only one that beats the best is ever
            // read further.
            let best_score = best.as_ref().map_or(f64::NEG_INFINITY, |best| best.score);
            let score = self.score(&transform, &mut scratch, best_score);
            if score <= best_score {
                continue;
            }
            let mut candidate = ScoredHypothesis {
                transform,
                score,
                inliers: mem::take(&mut scratch),
            };

            // Standard LO-RANSAC: only a new best is refined.
            if config.local_optimization && candidate.inliers.len() >= min_samples {
                self.local_optimization(&mut candidate, &mut lo_buffers);
            }

            let inlier_ratio = candidate.inliers.len() as f64 / n as f64;
            if inlier_ratio >= config.min_inlier_ratio {
                iteration_bound =
                    transforms::adaptive_iterations(inlier_ratio, min_samples, config.confidence)
                        .min(max_iter);
            }
            if let Some(previous) = best.replace(candidate) {
                scratch = previous.inliers;
            }
        }

        // A best with fewer inliers than a minimal sample has nothing to re-estimate from, and is
        // no model.
        let best_inlier_count = best.as_ref().map_or(0, |best| best.inliers.len());
        let Some(best) = best.filter(|best| best.inliers.len() >= min_samples) else {
            return Err(RansacFailure {
                reason: RansacFailureReason::NoInliersFound,
                iterations,
                best_inlier_count,
            });
        };
        lo_buffers
            .points
            .gather(&best.inliers, &self.pairs.reference, &self.pairs.target);
        let refined = transforms::estimate_transform(
            &lo_buffers.points.reference,
            &lo_buffers.points.target,
            self.transform_type,
        );
        if let Some(refined) = refined
            && self.estimator.is_plausible(&refined)
        {
            let refined_score = self.score(&refined, &mut scratch, best.score);
            if refined_score >= best.score && scratch.len() >= min_samples {
                return Ok(RansacResult {
                    transform: refined,
                    inliers: scratch,
                });
            }
        }
        Ok(RansacResult {
            transform: best.transform,
            inliers: best.inliers,
        })
    }

    /// Local optimization: refine `hypothesis` in place by re-estimating from its inliers and
    /// re-scoring, until the score stops improving or the iterations run out (LO-RANSAC).
    ///
    /// `hypothesis` is left exactly as it came in unless the refinement strictly improves it — see
    /// the acceptance test at the end for why that guard is not optional.
    fn local_optimization(&self, hypothesis: &mut ScoredHypothesis, buffers: &mut LocalOptBuffers) {
        let min_samples = self.transform_type.min_points();
        let mut current_transform = hypothesis.transform;

        // `inlier_buf` holds the current best's inliers, `scored` the refit being scored. The
        // hypothesis beat the running best, so its score is complete and is where LO starts.
        buffers.inlier_buf.clear();
        buffers.inlier_buf.extend_from_slice(&hypothesis.inliers);
        let mut current_score = hypothesis.score;

        for _ in 0..self.estimator.config.lo_iterations {
            if buffers.inlier_buf.len() < min_samples {
                break;
            }
            buffers.points.gather(
                &buffers.inlier_buf,
                &self.pairs.reference,
                &self.pairs.target,
            );
            let Some(refined) = transforms::estimate_transform(
                &buffers.points.reference,
                &buffers.points.target,
                self.transform_type,
            ) else {
                break;
            };
            let new_score = self.score(&refined, &mut buffers.scored, current_score);

            // A refit is taken on its score alone (Chum 2003). Scoring stops once the loss passes
            // the current score's, so a refit that did not beat it has a cut-short inlier list
            // whose count means nothing.
            if new_score <= current_score {
                break;
            }
            current_transform = refined;
            mem::swap(&mut buffers.inlier_buf, &mut buffers.scored);
            current_score = new_score;
        }

        // Commit only a refit that improved the score and stays plausible; otherwise `hypothesis`
        // keeps its complete pre-LO inliers.
        if current_score > hypothesis.score && self.estimator.is_plausible(&current_transform) {
            hypothesis.transform = current_transform;
            hypothesis.score = current_score;
            mem::swap(&mut hypothesis.inliers, &mut buffers.inlier_buf);
        }
    }

    /// The negated truncated Welsch loss of `transform` over the pairs — higher is better — with
    /// `inliers` filled with the pairs within the scorer's threshold.
    ///
    /// Scoring stops once the loss passes `−best_score`, where the hypothesis can no longer beat
    /// the best; `inliers` is then incomplete, and is only to be read when the returned score
    /// improves on `best_score`.
    #[inline]
    fn score(&self, transform: &Transform, inliers: &mut Vec<usize>, best_score: f64) -> f64 {
        inliers.clear();
        let mut total_loss = 0.0f64;
        let loss_budget = -best_score;

        for (i, (&r, &t)) in self
            .pairs
            .reference
            .iter()
            .zip(&self.pairs.target)
            .enumerate()
        {
            let dist_sq = (transform.apply(r) - t).length_squared();
            total_loss += self.scorer.loss(dist_sq);
            if total_loss > loss_budget {
                return -total_loss;
            }
            if self.scorer.is_inlier(dist_sq) {
                inliers.push(i);
            }
        }
        -total_loss
    }
}

/// Whether a minimal sample leaves the transform to the noise: two of its points closer than
/// `noise_scale`, or three whose triangle is lower than `noise_scale` over its longest side, so
/// they span no direction the noise does not blur. Every pair and every triplet is tested, so a
/// sample of four with three collinear among the last three is caught.
fn is_sample_degenerate(points: &[DVec2], noise_scale: f64) -> bool {
    let n = points.len();
    for i in 0..n {
        for j in i + 1..n {
            if (points[i] - points[j]).length() < noise_scale {
                return true;
            }
            for k in j + 1..n {
                let (a, b) = (points[j] - points[i], points[k] - points[i]);
                let longest = a
                    .length()
                    .max(b.length())
                    .max((points[k] - points[j]).length());
                // Twice the area over the longest side is the height onto it.
                if a.perp_dot(b).abs() / longest < noise_scale {
                    return true;
                }
            }
        }
    }
    false
}

#[cfg(feature = "internals")]
pub(crate) mod bench;
#[cfg(test)]
mod tests;
