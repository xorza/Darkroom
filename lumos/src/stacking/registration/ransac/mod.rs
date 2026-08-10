//! RANSAC (Random Sample Consensus) for robust transformation estimation.
//!
//! This module implements RANSAC with MAGSAC++ scoring to robustly estimate
//! transformations in the presence of outliers. MAGSAC++ (Barath & Matas 2020)
//! eliminates the need for manual threshold tuning by marginalizing over a
//! range of noise scales.
//!
//! The algorithm works by:
//! 1. Randomly sampling minimal point sets
//! 2. Computing candidate transformations
//! 3. Scoring with MAGSAC++ (continuous likelihood, not binary inlier/outlier)
//! 4. Keeping the best model
//! 5. Refining with least squares on inliers

#[cfg(test)]
mod tests;

pub(crate) mod config;
mod magsac;
mod sampling;
pub(super) mod transforms;

use magsac::MagsacScorer;
use transforms::{adaptive_iterations, estimate_transform};

use std::cmp::Ordering;

use glam::DVec2;

use crate::stacking::registration::ransac::config::RansacConfig;
use crate::stacking::registration::ransac::sampling::{
    PHASE_POOL_FRACTIONS, PHASE_WEIGHTED, SAMPLING_PHASES, make_rng, random_sample_into,
    weighted_sample_into,
};
use crate::stacking::registration::transform::{Transform, TransformType};
use crate::stacking::registration::triangle::voting::PointMatch;

/// Pre-allocated buffers for local optimization (LO-RANSAC) to avoid per-iteration allocations.
#[derive(Debug)]
struct LocalOptBuffers {
    inlier_buf: Vec<usize>,
    point_buf_ref: Vec<DVec2>,
    point_buf_target: Vec<DVec2>,
}

impl LocalOptBuffers {
    fn with_capacity(n: usize) -> Self {
        Self {
            inlier_buf: Vec::with_capacity(n),
            point_buf_ref: Vec::with_capacity(n),
            point_buf_target: Vec::with_capacity(n),
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

impl ScoredHypothesis {
    /// An empty hypothesis, scored worse than anything that will be compared against it.
    ///
    /// `transform` is a placeholder: "nothing found yet" is carried by `inliers` staying below
    /// the model's minimum sample count, and no reader reaches the transform without clearing
    /// that bar first.
    fn empty(inlier_capacity: usize) -> Self {
        Self {
            transform: Transform::identity(),
            score: f64::NEG_INFINITY,
            inliers: Vec::with_capacity(inlier_capacity),
        }
    }
}

/// Minimum cross-product magnitude to consider points non-collinear.
/// For points separated by ~1 pixel, a cross product of 1.0 corresponds
/// to ~1 pixel perpendicular offset — below this, the sample is too
/// close to a line for reliable transform estimation.
const COLLINEARITY_THRESHOLD: f64 = 1.0;

/// Result of RANSAC estimation.
#[derive(Debug, Clone)]
pub(super) struct RansacResult {
    /// Best transformation found.
    pub(super) transform: Transform,
    /// Indices of inlier matches.
    pub(super) inliers: Vec<usize>,
    /// RANSAC iterations performed — a diagnostic; the adaptive-early-termination
    /// test asserts on it (no production reader yet).
    #[allow(dead_code)]
    iterations: usize,
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
        assert!(max_sigma.is_finite() && max_sigma > 0.0);
        Self { config, max_sigma }
    }

    /// Check whether a transform hypothesis is physically plausible.
    ///
    /// Rejects hypotheses where rotation or scale fall outside configured bounds.
    /// Returns `true` if the transform is plausible (or checks are disabled).
    fn is_plausible(&self, transform: &Transform) -> bool {
        if let Some(max_rotation) = self.config.max_rotation {
            let angle = transform.rotation_angle().abs();
            if angle > max_rotation {
                return false;
            }
        }
        if let Some((min_scale, max_scale)) = self.config.scale_range {
            let scale = transform.scale_factor();
            if scale < min_scale || scale > max_scale {
                return false;
            }
        }
        true
    }

    /// Local optimization: refine `hypothesis` in place by iterative re-estimation (LO-RANSAC).
    ///
    /// 1. Re-estimate transform using current inliers
    /// 2. Find new inliers with the refined transform
    /// 3. Repeat until convergence or max iterations
    ///
    /// Typically improves inlier count by 5-15%. `hypothesis` is left exactly as it came in
    /// unless the refinement strictly improves it — see the acceptance test at the end for why
    /// that guard is not optional.
    fn local_optimization(
        &self,
        ref_points: &[DVec2],
        target_points: &[DVec2],
        hypothesis: &mut ScoredHypothesis,
        scorer: &MagsacScorer,
        buffers: &mut LocalOptBuffers,
    ) {
        let transform_type = hypothesis.transform.transform_type();
        let min_samples = transform_type.min_points();
        let mut current_transform = hypothesis.transform;

        // Use inlier_buf as the "current best" and a local scratch for scoring.
        buffers.inlier_buf.clear();
        buffers.inlier_buf.extend_from_slice(&hypothesis.inliers);
        let mut scratch_inliers = Vec::with_capacity(buffers.inlier_buf.len());

        // Compute initial score
        let initial_score = score_hypothesis(
            ref_points,
            target_points,
            &current_transform,
            scorer,
            &mut scratch_inliers,
            f64::NEG_INFINITY,
        );
        let mut current_score = initial_score;

        for _ in 0..self.config.lo_iterations {
            if buffers.inlier_buf.len() < min_samples {
                break;
            }

            // Re-estimate transform using all current inliers
            buffers.point_buf_ref.clear();
            buffers.point_buf_target.clear();
            for &i in buffers.inlier_buf.iter() {
                buffers.point_buf_ref.push(ref_points[i]);
                buffers.point_buf_target.push(target_points[i]);
            }

            let refined = match estimate_transform(
                &buffers.point_buf_ref,
                &buffers.point_buf_target,
                transform_type,
            ) {
                Some(t) => t,
                None => break,
            };

            // Score with refined transform
            let new_score = score_hypothesis(
                ref_points,
                target_points,
                &refined,
                scorer,
                &mut scratch_inliers,
                current_score,
            );

            // Check for convergence (no improvement)
            if scratch_inliers.len() <= buffers.inlier_buf.len() && new_score <= current_score {
                break;
            }

            // Update if improved
            current_transform = refined;
            std::mem::swap(&mut buffers.inlier_buf, &mut scratch_inliers);
            current_score = new_score;
        }

        // Commit only if LO actually improved the score (and is still plausible). Without the
        // `>` guard, LO can hand back a lower score — it accepts refits with more (possibly
        // budget-early-exited) inliers even when the score drops — discarding a hypothesis that
        // had already beaten the running best. Leaving `hypothesis` untouched on that path is
        // what keeps its complete pre-LO inliers.
        if current_score > hypothesis.score && self.is_plausible(&current_transform) {
            hypothesis.transform = current_transform;
            hypothesis.score = current_score;
            std::mem::swap(&mut hypothesis.inliers, &mut buffers.inlier_buf);
        }
    }

    /// Core RANSAC loop with MAGSAC++ scoring.
    ///
    /// The `sample_fn` closure fills `sample_indices` buffer each iteration.
    /// It receives `(iteration, max_iterations, &mut sample_buf)`.
    fn ransac_loop(
        &self,
        ref_points: &[DVec2],
        target_points: &[DVec2],
        n: usize,
        min_samples: usize,
        transform_type: TransformType,
        mut sample_fn: impl FnMut(usize, usize, &mut Vec<usize>),
    ) -> Option<RansacResult> {
        // Initialize MAGSAC++ scorer
        let scorer = MagsacScorer::new(self.max_sigma);

        let mut best = ScoredHypothesis::empty(0);

        // Pre-allocate buffers to avoid per-iteration allocations
        let mut sample_indices: Vec<usize> = Vec::with_capacity(min_samples);
        let mut sample_ref: Vec<DVec2> = Vec::with_capacity(min_samples);
        let mut sample_target: Vec<DVec2> = Vec::with_capacity(min_samples);
        let mut current = ScoredHypothesis::empty(n);
        let mut lo_buffers = LocalOptBuffers::with_capacity(n);

        let mut iterations = 0;
        let max_iter = self.config.max_iterations;

        while iterations < max_iter {
            iterations += 1;

            // Fill sample indices via the provided strategy
            sample_fn(iterations, max_iter, &mut sample_indices);

            // Extract sample points (reusing buffers)
            sample_ref.clear();
            sample_target.clear();
            for &i in &sample_indices {
                sample_ref.push(ref_points[i]);
                sample_target.push(target_points[i]);
            }

            // Skip degenerate samples (coincident/collinear points in either image)
            if is_sample_degenerate(&sample_ref) || is_sample_degenerate(&sample_target) {
                continue;
            }

            // Estimate transformation from sample
            let transform = match estimate_transform(&sample_ref, &sample_target, transform_type) {
                Some(t) => t,
                None => continue,
            };

            // Reject physically implausible hypotheses early (before expensive scoring)
            if !self.is_plausible(&transform) {
                continue;
            }

            // Score with MAGSAC++ (preemptive: skip if cannot beat current best)
            current.transform = transform;
            current.score = score_hypothesis(
                ref_points,
                target_points,
                &transform,
                &scorer,
                &mut current.inliers,
                best.score,
            );

            // Local Optimization: refine only new-best hypotheses (standard LO-RANSAC)
            if self.config.local_optimization
                && current.score > best.score
                && current.inliers.len() >= min_samples
            {
                self.local_optimization(
                    ref_points,
                    target_points,
                    &mut current,
                    &scorer,
                    &mut lo_buffers,
                );
            }

            // Update best if improved. The swap hands `current` the old best's inlier buffer,
            // which the next iteration's scoring refills.
            if current.score > best.score {
                std::mem::swap(&mut best, &mut current);

                // Adaptive iteration count based on inlier ratio
                let inlier_ratio = best.inliers.len() as f64 / n as f64;
                if inlier_ratio >= self.config.min_inlier_ratio {
                    let adaptive_max =
                        adaptive_iterations(inlier_ratio, min_samples, self.config.confidence);
                    if iterations >= adaptive_max {
                        break;
                    }
                }
            }
        }

        // Final refinement with least squares on all inliers. Too few inliers to re-estimate
        // from is also how "no hypothesis was ever accepted" reads — `best` starts empty.
        if best.inliers.len() >= min_samples {
            lo_buffers.point_buf_ref.clear();
            lo_buffers.point_buf_target.clear();
            for &i in &best.inliers {
                lo_buffers.point_buf_ref.push(ref_points[i]);
                lo_buffers.point_buf_target.push(target_points[i]);
            }

            let refined = estimate_transform(
                &lo_buffers.point_buf_ref,
                &lo_buffers.point_buf_target,
                transform_type,
            );

            // The loop's scratch, reused to score the refit.
            let mut scratch_inliers = current.inliers;

            if let Some(refined) = refined
                && refined.is_valid()
                && self.is_plausible(&refined)
            {
                let refined_score = score_hypothesis(
                    ref_points,
                    target_points,
                    &refined,
                    &scorer,
                    &mut scratch_inliers,
                    best.score,
                );

                if refined_score >= best.score && scratch_inliers.len() >= min_samples {
                    return Some(RansacResult {
                        transform: refined,
                        inliers: scratch_inliers,
                        iterations,
                    });
                }
            }

            return Some(RansacResult {
                transform: best.transform,
                inliers: best.inliers,
                iterations,
            });
        }

        None
    }

    /// Estimate transformation from star matches.
    ///
    /// Uses match confidence scores to guide hypothesis sampling via 3-phase
    /// progressive sampling: early iterations preferentially sample high-confidence
    /// matches, converging faster than uniform random sampling.
    ///
    /// # Arguments
    /// * `matches` - Star matches with confidence scores from triangle matching
    /// * `ref_stars` - Reference star positions
    /// * `target_stars` - Target star positions
    /// * `transform_type` - Type of transformation to estimate
    ///
    /// # Returns
    /// Best transformation found, or None if estimation failed.
    pub(super) fn estimate(
        &self,
        matches: &[PointMatch],
        ref_stars: &[DVec2],
        target_stars: &[DVec2],
        transform_type: TransformType,
    ) -> Option<RansacResult> {
        if matches.is_empty() {
            return None;
        }

        // Extract point pairs and confidences from matches
        let ref_points: Vec<DVec2> = matches.iter().map(|m| ref_stars[m.ref_idx]).collect();
        let target_points: Vec<DVec2> =
            matches.iter().map(|m| target_stars[m.target_idx]).collect();
        let confidences: Vec<f64> = matches.iter().map(|m| m.confidence).collect();

        let n = ref_points.len();
        let min_samples = transform_type.min_points();

        if n < min_samples {
            return None;
        }

        let mut rng = make_rng(self.config.seed);

        // Build sorted index by confidence (descending)
        let mut sorted_indices: Vec<usize> = (0..n).collect();
        sorted_indices.sort_by(|&a, &b| {
            confidences[b]
                .partial_cmp(&confidences[a])
                .unwrap_or(Ordering::Equal)
        });

        // Compute weights for weighted sampling
        // Higher confidence = higher probability of being sampled
        let weights: Vec<f64> = confidences
            .iter()
            .map(|&c| (c + 0.1).powi(2)) // Square to emphasize high-confidence matches
            .collect();

        // Persistent index array for Fisher-Yates shuffle (avoids O(n) re-init per iteration)
        let mut shuffle_indices: Vec<usize> = Vec::new();
        // Persistent key buffer for weighted A-Res sampling (avoids a per-iteration allocation).
        let mut weighted_scratch: Vec<(usize, f64)> = Vec::new();

        self.ransac_loop(
            &ref_points,
            &target_points,
            n,
            min_samples,
            transform_type,
            |iteration, max_iter, sample_buf| {
                // Progressive sampling: phases ramp from high-confidence pool to full pool
                let phase = (iteration * SAMPLING_PHASES / max_iter).min(SAMPLING_PHASES - 1);
                let pool_size =
                    ((n as f64 * PHASE_POOL_FRACTIONS[phase]).ceil() as usize).max(min_samples);
                let use_weighted = PHASE_WEIGHTED[phase];

                if use_weighted {
                    weighted_sample_into(
                        &mut rng,
                        &sorted_indices[..pool_size],
                        &weights,
                        min_samples,
                        sample_buf,
                        &mut weighted_scratch,
                    );
                } else {
                    random_sample_into(&mut rng, n, min_samples, sample_buf, &mut shuffle_indices);
                }
            },
        )
    }
}

/// Check if a sample of points is degenerate (too close together or collinear).
///
/// For 2 points: checks if they are nearly coincident.
/// For 3+ points: checks if any pair is nearly coincident or if all points are collinear.
fn is_sample_degenerate(points: &[DVec2]) -> bool {
    const MIN_DIST_SQ: f64 = 1.0; // Minimum 1 pixel apart

    let n = points.len();
    if n < 2 {
        return false;
    }

    // Check all pairs for near-coincidence
    for i in 0..n {
        for j in (i + 1)..n {
            if (points[i] - points[j]).length_squared() < MIN_DIST_SQ {
                return true;
            }
        }
    }

    // For 3+ points, check collinearity via cross product
    if n >= 3 {
        let v0 = points[1] - points[0];
        let mut all_collinear = true;
        for p in &points[2..] {
            let v = *p - points[0];
            let cross = v0.x * v.y - v0.y * v.x;
            if cross.abs() > COLLINEARITY_THRESHOLD {
                all_collinear = false;
                break;
            }
        }
        if all_collinear {
            return true;
        }
    }

    false
}

/// Score a hypothesis using MAGSAC++ scoring.
///
/// Returns negative total loss (higher score = better model).
/// Also populates the inliers buffer with indices of points within threshold.
///
/// When `best_score` is provided, exits early once the cumulative loss exceeds
/// `-best_score` (the hypothesis cannot beat the current best). On early exit,
/// the inliers buffer is incomplete — callers must only use it when the returned
/// score improves on `best_score`.
#[inline]
fn score_hypothesis(
    ref_points: &[DVec2],
    target_points: &[DVec2],
    transform: &Transform,
    scorer: &MagsacScorer,
    inliers: &mut Vec<usize>,
    best_score: f64,
) -> f64 {
    inliers.clear();
    let mut total_loss = 0.0f64;
    let loss_budget = -best_score;

    for (i, (r, t)) in ref_points.iter().zip(target_points.iter()).enumerate() {
        let p = transform.apply(*r);
        let dist_sq = (p - *t).length_squared();

        total_loss += scorer.loss(dist_sq);
        if total_loss > loss_budget {
            return -total_loss;
        }
        if scorer.is_inlier(dist_sq) {
            inliers.push(i);
        }
    }

    // Negate so higher score = better model
    -total_loss
}
