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

mod magsac;
pub(crate) mod transforms;

use std::cmp::Ordering;

use magsac::MagsacScorer;
use transforms::{adaptive_iterations, estimate_transform};

use glam::DVec2;
use rand::prelude::*;

use crate::stacking::registration::result::RegistrationError;
use crate::stacking::registration::transform::{Transform, TransformType};
use crate::stacking::registration::triangle::voting::PointMatch;

/// Progressive sampling: 3 phases from high-confidence pool to full pool.
/// This front-loads good candidates, improving early convergence.
const SAMPLING_PHASES: usize = 3;
/// Pool fraction per phase: top 25% → top 50% → full pool.
const PHASE_POOL_FRACTIONS: [f64; 3] = [0.25, 0.50, 1.0];
/// Whether each phase uses weighted sampling (vs uniform random).
const PHASE_WEIGHTED: [bool; 3] = [true, true, false];

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

/// Minimum cross-product magnitude to consider points non-collinear.
/// For points separated by ~1 pixel, a cross product of 1.0 corresponds
/// to ~1 pixel perpendicular offset — below this, the sample is too
/// close to a line for reliable transform estimation.
const COLLINEARITY_THRESHOLD: f64 = 1.0;

/// Configuration for robust transform estimation.
#[derive(Debug, Clone)]
pub struct RansacConfig {
    /// Maximum hypotheses to evaluate. Default: 2000.
    pub max_iterations: usize,
    /// Target confidence for adaptive early termination. Default: 0.995.
    pub confidence: f64,
    /// Minimum inlier ratio before adaptive early termination. Default: 0.3.
    pub min_inlier_ratio: f64,
    /// Random seed for reproducible sampling. Default: random.
    pub seed: Option<u64>,
    /// Whether to refine promising hypotheses with LO-RANSAC. Default: true.
    pub local_optimization: bool,
    /// Maximum LO-RANSAC refinement iterations. Default: 10.
    pub lo_iterations: usize,
    /// Maximum absolute rotation in radians. Default: 10 degrees.
    pub max_rotation: Option<f64>,
    /// Accepted uniform-scale range. Default: 0.8 to 1.2.
    pub scale_range: Option<(f64, f64)>,
}

impl Default for RansacConfig {
    fn default() -> Self {
        Self {
            max_iterations: 2000,
            confidence: 0.995,
            min_inlier_ratio: 0.3,
            seed: None,
            local_optimization: true,
            lo_iterations: 10,
            max_rotation: Some(10.0_f64.to_radians()),
            scale_range: Some((0.8, 1.2)),
        }
    }
}

impl RansacConfig {
    pub(crate) fn validate(&self) -> Result<(), RegistrationError> {
        let invalid = |msg: String| Err(RegistrationError::InvalidConfig(msg));
        if self.max_iterations == 0 {
            return invalid(format!(
                "ransac max_iterations must be positive, got {}",
                self.max_iterations
            ));
        }
        if self.local_optimization && self.lo_iterations == 0 {
            return invalid(format!(
                "ransac lo_iterations must be positive when local_optimization is enabled, got {}",
                self.lo_iterations
            ));
        }
        if !(0.0..=1.0).contains(&self.confidence) {
            return invalid(format!(
                "ransac confidence must be in [0, 1], got {}",
                self.confidence
            ));
        }
        if !(self.min_inlier_ratio > 0.0 && self.min_inlier_ratio <= 1.0) {
            return invalid(format!(
                "ransac min_inlier_ratio must be in (0, 1], got {}",
                self.min_inlier_ratio
            ));
        }
        if let Some(max_rotation) = self.max_rotation
            && (!max_rotation.is_finite() || max_rotation <= 0.0)
        {
            return invalid(format!(
                "ransac max_rotation must be positive and finite, got {max_rotation}"
            ));
        }
        if let Some((min_scale, max_scale)) = self.scale_range {
            if !min_scale.is_finite() || !max_scale.is_finite() {
                return invalid(format!(
                    "ransac scale_range bounds must be finite, got ({min_scale}, {max_scale})"
                ));
            }
            if !(min_scale > 0.0 && max_scale > min_scale) {
                return invalid(format!(
                    "ransac scale_range must have 0 < min < max, got ({min_scale}, {max_scale})"
                ));
            }
        }
        Ok(())
    }
}

/// Create a ChaCha8Rng from an optional seed.
///
/// When `seed` is `None`, seeds from `thread_rng()` for non-deterministic behavior.
/// Always using ChaCha8Rng avoids enum dispatch overhead on every RNG call.
fn make_rng(seed: Option<u64>) -> rand_chacha::ChaCha8Rng {
    use rand_chacha::rand_core::SeedableRng;
    match seed {
        Some(s) => rand_chacha::ChaCha8Rng::seed_from_u64(s),
        None => rand_chacha::ChaCha8Rng::seed_from_u64(rand::rng().next_u64()),
    }
}

/// Result of RANSAC estimation.
#[derive(Debug, Clone)]
pub(crate) struct RansacResult {
    /// Best transformation found.
    pub(crate) transform: Transform,
    /// Indices of inlier matches.
    pub(crate) inliers: Vec<usize>,
    /// RANSAC iterations performed — a diagnostic; the adaptive-early-termination
    /// test asserts on it (no production reader yet).
    #[allow(dead_code)]
    pub(crate) iterations: usize,
}

/// RANSAC estimator for robust transformation fitting.
#[derive(Debug)]
pub(crate) struct RansacEstimator {
    config: RansacConfig,
    max_sigma: f64,
}

impl RansacEstimator {
    /// Create a RANSAC estimator for the runtime-derived maximum noise scale.
    pub(crate) fn new(config: RansacConfig, max_sigma: f64) -> Self {
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

    /// Local optimization: refine transform using iterative re-estimation (LO-RANSAC).
    ///
    /// 1. Re-estimate transform using current inliers
    /// 2. Find new inliers with the refined transform
    /// 3. Repeat until convergence or max iterations
    ///
    /// On return, `inlier_buf` contains the best inlier set found.
    /// Typically improves inlier count by 5-15%.
    fn local_optimization(
        &self,
        ref_points: &[DVec2],
        target_points: &[DVec2],
        initial_transform: &Transform,
        initial_inliers: &[usize],
        scorer: &MagsacScorer,
        buffers: &mut LocalOptBuffers,
    ) -> (Transform, f64) {
        let transform_type = initial_transform.transform_type();
        let min_samples = transform_type.min_points();
        let mut current_transform = *initial_transform;

        // Use inlier_buf as the "current best" and a local scratch for scoring.
        buffers.inlier_buf.clear();
        buffers.inlier_buf.extend_from_slice(initial_inliers);
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

        (current_transform, current_score)
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

        let mut best_transform: Option<Transform> = None;
        let mut best_inliers: Vec<usize> = Vec::new();
        let mut best_score = f64::NEG_INFINITY;

        // Pre-allocate buffers to avoid per-iteration allocations
        let mut sample_indices: Vec<usize> = Vec::with_capacity(min_samples);
        let mut sample_ref: Vec<DVec2> = Vec::with_capacity(min_samples);
        let mut sample_target: Vec<DVec2> = Vec::with_capacity(min_samples);
        let mut inlier_buf: Vec<usize> = Vec::with_capacity(n);
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
            let mut score = score_hypothesis(
                ref_points,
                target_points,
                &transform,
                &scorer,
                &mut inlier_buf,
                best_score,
            );

            let mut current_transform = transform;

            // Local Optimization: refine only new-best hypotheses (standard LO-RANSAC)
            if self.config.local_optimization
                && score > best_score
                && inlier_buf.len() >= min_samples
            {
                let (lo_transform, lo_score) = self.local_optimization(
                    ref_points,
                    target_points,
                    &current_transform,
                    &inlier_buf,
                    &scorer,
                    &mut lo_buffers,
                );
                // Accept LO only if it actually improved the score (and is still plausible).
                // Without the `lo_score > score` guard, LO can return a lower score (it accepts
                // refits with more — possibly budget-early-exited — inliers even when the score
                // drops), discarding a hypothesis that had already beaten `best_score`. On the
                // not-accepted path `inlier_buf` still holds the complete pre-LO inliers.
                if lo_score > score && self.is_plausible(&lo_transform) {
                    current_transform = lo_transform;
                    std::mem::swap(&mut inlier_buf, &mut lo_buffers.inlier_buf);
                    score = lo_score;
                }
            }

            // Update best if improved
            if score > best_score {
                best_score = score;
                std::mem::swap(&mut best_inliers, &mut inlier_buf);
                best_transform = Some(current_transform);

                // Adaptive iteration count based on inlier ratio
                let inlier_ratio = best_inliers.len() as f64 / n as f64;
                if inlier_ratio >= self.config.min_inlier_ratio {
                    let adaptive_max =
                        adaptive_iterations(inlier_ratio, min_samples, self.config.confidence);
                    if iterations >= adaptive_max {
                        break;
                    }
                }
            }
        }

        // Final refinement with least squares on all inliers
        if let Some(transform) = best_transform
            && best_inliers.len() >= min_samples
        {
            lo_buffers.point_buf_ref.clear();
            lo_buffers.point_buf_target.clear();
            for &i in &best_inliers {
                lo_buffers.point_buf_ref.push(ref_points[i]);
                lo_buffers.point_buf_target.push(target_points[i]);
            }

            let refined = estimate_transform(
                &lo_buffers.point_buf_ref,
                &lo_buffers.point_buf_target,
                transform_type,
            );

            if let Some(refined) = refined
                && refined.is_valid()
                && self.is_plausible(&refined)
            {
                let refined_score = score_hypothesis(
                    ref_points,
                    target_points,
                    &refined,
                    &scorer,
                    &mut inlier_buf,
                    best_score,
                );

                if refined_score >= best_score && inlier_buf.len() >= min_samples {
                    return Some(RansacResult {
                        transform: refined,
                        inliers: inlier_buf,
                        iterations,
                    });
                }
            }

            return Some(RansacResult {
                transform,
                inliers: best_inliers,
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
    pub(crate) fn estimate(
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

/// Weighted sampling of k unique indices from a pool.
///
/// Samples indices with probability proportional to their weights using
/// Algorithm A-Res (reservoir sampling with weights). Uses `select_nth_unstable`
/// for O(n) average-case partitioning instead of a full O(n log n) sort.
fn weighted_sample_into<R: Rng>(
    rng: &mut R,
    pool: &[usize],
    weights: &[f64],
    k: usize,
    buffer: &mut Vec<usize>,
    scratch: &mut Vec<(usize, f64)>,
) {
    buffer.clear();

    if pool.len() <= k {
        buffer.extend_from_slice(pool);
        return;
    }

    // Use reservoir sampling with weights (Algorithm A-Res)
    // For each item, compute key = random^(1/weight), keep top k keys.
    // `scratch` is reused across iterations to avoid a per-iteration allocation.
    scratch.clear();
    scratch.extend(pool.iter().map(|&idx| {
        // `weights` has one entry per point and `idx` indexes the same `0..n` pool, so it can't miss.
        let w = weights[idx].max(0.001);
        let u: f64 = rng.random();
        let key = u.powf(1.0 / w); // Higher weight = higher expected key
        (idx, key)
    }));

    // Partition so the top k elements (by descending key) are in [0..k]
    scratch.select_nth_unstable_by(k - 1, |a, b| {
        b.1.partial_cmp(&a.1).unwrap_or(Ordering::Equal)
    });

    for &(idx, _) in &scratch[..k] {
        buffer.push(idx);
    }
}

/// Randomly sample k unique indices from 0..n into pre-allocated buffer.
///
/// Uses partial Fisher-Yates shuffle: O(k) time. The `indices` scratch buffer
/// persists across calls to avoid re-creating the `[0..n]` array each iteration.
/// After sampling, the swaps are undone to restore `indices` to `[0..n]`.
fn random_sample_into<R: Rng>(
    rng: &mut R,
    n: usize,
    k: usize,
    buffer: &mut Vec<usize>,
    indices: &mut Vec<usize>,
) {
    debug_assert!(k <= n, "Cannot sample {} indices from {}", k, n);

    // Initialize or resize the persistent index array
    if indices.len() != n {
        indices.clear();
        indices.extend(0..n);
    }

    // Partial Fisher-Yates: shuffle first k elements, recording swap targets
    buffer.clear();
    // k is always small (2-4 for RANSAC min_samples), stack array suffices
    let mut swap_targets = [0usize; 8];
    assert!(
        k <= swap_targets.len(),
        "k={k} exceeds swap tracking capacity"
    );
    for i in 0..k {
        let j = rng.random_range(i..n);
        indices.swap(i, j);
        swap_targets[i] = j;
        buffer.push(indices[i]);
    }

    // Undo swaps in reverse order to restore indices to [0, 1, 2, ..., n-1]
    for i in (0..k).rev() {
        indices.swap(i, swap_targets[i]);
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
