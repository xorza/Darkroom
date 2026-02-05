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
mod transforms;

pub(crate) use transforms::{
    adaptive_iterations, centroid, estimate_affine, estimate_homography, estimate_similarity,
    estimate_transform, normalize_points,
};

use magsac::MagsacScorer;

use glam::DVec2;
use rand::prelude::*;

use crate::registration::transform::{Transform, TransformType};
use crate::registration::triangle::PointMatch;

/// RANSAC parameters extracted from Config.
#[derive(Debug, Clone)]
pub struct RansacParams {
    pub max_iterations: usize,
    /// Maximum noise scale (σ_max) in pixels for MAGSAC++ scoring.
    ///
    /// Points with residuals greater than ~3·max_sigma are treated as outliers.
    /// The effective threshold is approximately `3.03 * max_sigma` (based on
    /// the 99% χ² quantile for 2 degrees of freedom).
    ///
    /// Default: 1.0 pixel (~3px effective threshold).
    ///
    /// Migration from old `inlier_threshold`: use `max_sigma = inlier_threshold / 3.0`
    pub max_sigma: f64,
    pub confidence: f64,
    pub min_inlier_ratio: f64,
    pub seed: Option<u64>,
    pub use_local_optimization: bool,
    pub lo_max_iterations: usize,
    pub max_rotation: Option<f64>,
    pub scale_range: Option<(f64, f64)>,
}

impl Default for RansacParams {
    fn default() -> Self {
        Self {
            max_iterations: 2000,
            max_sigma: 1.0, // ~3px effective threshold
            confidence: 0.999,
            min_inlier_ratio: 0.3,
            seed: None,
            use_local_optimization: true,
            lo_max_iterations: 10,
            max_rotation: None,
            scale_range: None,
        }
    }
}

// Wrapper for seeded vs non-seeded RNG.
// ChaCha8Rng is 304 bytes; Box avoids large enum variant size difference.
enum RngWrapper {
    Seeded(Box<rand_chacha::ChaCha8Rng>),
    Thread(rand::rngs::ThreadRng),
}

impl RngWrapper {
    fn new(seed: Option<u64>) -> Self {
        match seed {
            Some(s) => {
                use rand_chacha::rand_core::SeedableRng;
                RngWrapper::Seeded(Box::new(rand_chacha::ChaCha8Rng::seed_from_u64(s)))
            }
            None => RngWrapper::Thread(rand::rng()),
        }
    }
}

impl rand::RngCore for RngWrapper {
    fn next_u32(&mut self) -> u32 {
        use rand_chacha::rand_core::Rng;
        match self {
            RngWrapper::Seeded(rng) => rng.next_u32(),
            RngWrapper::Thread(rng) => rand::RngCore::next_u32(rng),
        }
    }

    fn next_u64(&mut self) -> u64 {
        use rand_chacha::rand_core::Rng;
        match self {
            RngWrapper::Seeded(rng) => rng.next_u64(),
            RngWrapper::Thread(rng) => rand::RngCore::next_u64(rng),
        }
    }

    fn fill_bytes(&mut self, dest: &mut [u8]) {
        use rand_chacha::rand_core::Rng;
        match self {
            RngWrapper::Seeded(rng) => rng.fill_bytes(dest),
            RngWrapper::Thread(rng) => rand::RngCore::fill_bytes(rng, dest),
        }
    }
}

/// Result of RANSAC estimation.
#[derive(Debug, Clone)]
pub struct RansacResult {
    /// Best transformation found.
    pub transform: Transform,
    /// Indices of inlier matches.
    pub inliers: Vec<usize>,
    /// Number of iterations performed. Used for diagnostics and testing.
    #[allow(dead_code)]
    pub iterations: usize,
    /// Final inlier ratio. Used for diagnostics and testing.
    #[allow(dead_code)]
    pub inlier_ratio: f64,
}

/// RANSAC estimator for robust transformation fitting.
pub struct RansacEstimator {
    params: RansacParams,
}

impl RansacEstimator {
    /// Create a new RANSAC estimator.
    pub fn new(params: RansacParams) -> Self {
        Self { params }
    }

    /// Check whether a transform hypothesis is physically plausible.
    ///
    /// Rejects hypotheses where rotation or scale fall outside configured bounds.
    /// Returns `true` if the transform is plausible (or checks are disabled).
    fn is_plausible(&self, transform: &Transform) -> bool {
        if let Some(max_rotation) = self.params.max_rotation {
            let angle = transform.rotation_angle().abs();
            if angle > max_rotation {
                return false;
            }
        }
        if let Some((min_scale, max_scale)) = self.params.scale_range {
            let scale = transform.scale_factor();
            if scale < min_scale || scale > max_scale {
                return false;
            }
        }
        true
    }

    /// Perform local optimization on a promising hypothesis.
    ///
    /// This implements the LO-RANSAC algorithm:
    /// 1. Re-estimate transform using current inliers
    /// 2. Find new inliers with the refined transform
    /// 3. Repeat until convergence or max iterations
    ///
    /// This typically improves the inlier count by 5-15%.
    #[allow(clippy::too_many_arguments)]
    fn local_optimization(
        &self,
        ref_points: &[DVec2],
        target_points: &[DVec2],
        initial_transform: &Transform,
        initial_inliers: &[usize],
        scorer: &MagsacScorer,
        inlier_buf: &mut Vec<usize>,
        point_buf_ref: &mut Vec<DVec2>,
        point_buf_target: &mut Vec<DVec2>,
    ) -> (Transform, Vec<usize>, f64) {
        let transform_type = initial_transform.transform_type;
        let min_samples = transform_type.min_points();
        let mut current_transform = *initial_transform;
        let mut current_inliers = initial_inliers.to_vec();

        // Compute initial score
        let initial_score = score_hypothesis(
            ref_points,
            target_points,
            &current_transform,
            scorer,
            inlier_buf,
        );
        let mut current_score = initial_score;

        for _ in 0..self.params.lo_max_iterations {
            if current_inliers.len() < min_samples {
                break;
            }

            // Re-estimate transform using all current inliers
            point_buf_ref.clear();
            point_buf_target.clear();
            for &i in &current_inliers {
                point_buf_ref.push(ref_points[i]);
                point_buf_target.push(target_points[i]);
            }

            let refined = match estimate_transform(point_buf_ref, point_buf_target, transform_type)
            {
                Some(t) => t,
                None => break,
            };

            // Score with refined transform
            let new_score =
                score_hypothesis(ref_points, target_points, &refined, scorer, inlier_buf);

            // Check for convergence (no improvement)
            if inlier_buf.len() <= current_inliers.len() && new_score <= current_score {
                break;
            }

            // Update if improved
            current_transform = refined;
            std::mem::swap(&mut current_inliers, inlier_buf);
            current_score = new_score;
        }

        (current_transform, current_inliers, current_score)
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
        let scorer = MagsacScorer::new(self.params.max_sigma);

        let mut best_transform: Option<Transform> = None;
        let mut best_inliers: Vec<usize> = Vec::new();
        let mut best_score = f64::NEG_INFINITY;

        // Pre-allocate buffers to avoid per-iteration allocations
        let mut sample_indices: Vec<usize> = Vec::with_capacity(min_samples);
        let mut sample_ref: Vec<DVec2> = Vec::with_capacity(min_samples);
        let mut sample_target: Vec<DVec2> = Vec::with_capacity(min_samples);
        let mut inlier_buf: Vec<usize> = Vec::with_capacity(n);
        let mut lo_inlier_buf: Vec<usize> = Vec::with_capacity(n);
        let mut lo_point_buf_ref: Vec<DVec2> = Vec::with_capacity(n);
        let mut lo_point_buf_target: Vec<DVec2> = Vec::with_capacity(n);

        let mut iterations = 0;
        let max_iter = self.params.max_iterations;

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

            // Skip degenerate samples (coincident/collinear points)
            if is_sample_degenerate(&sample_ref) {
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

            // Score with MAGSAC++
            let mut score = score_hypothesis(
                ref_points,
                target_points,
                &transform,
                &scorer,
                &mut inlier_buf,
            );

            let mut current_transform = transform;

            // Local Optimization: if this hypothesis looks promising, refine it
            if self.params.use_local_optimization && inlier_buf.len() >= min_samples {
                let (lo_transform, lo_inliers, lo_score) = self.local_optimization(
                    ref_points,
                    target_points,
                    &current_transform,
                    &inlier_buf,
                    &scorer,
                    &mut lo_inlier_buf,
                    &mut lo_point_buf_ref,
                    &mut lo_point_buf_target,
                );
                // Only accept LO result if it's still plausible
                if self.is_plausible(&lo_transform) {
                    current_transform = lo_transform;
                    inlier_buf = lo_inliers;
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
                if inlier_ratio >= self.params.min_inlier_ratio {
                    let adaptive_max =
                        adaptive_iterations(inlier_ratio, min_samples, self.params.confidence);
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
            lo_point_buf_ref.clear();
            lo_point_buf_target.clear();
            for &i in &best_inliers {
                lo_point_buf_ref.push(ref_points[i]);
                lo_point_buf_target.push(target_points[i]);
            }

            let refined =
                estimate_transform(&lo_point_buf_ref, &lo_point_buf_target, transform_type)
                    .unwrap_or(transform);

            score_hypothesis(
                ref_points,
                target_points,
                &refined,
                &scorer,
                &mut inlier_buf,
            );

            let inlier_ratio = inlier_buf.len() as f64 / n as f64;

            return Some(RansacResult {
                transform: refined,
                inliers: inlier_buf,
                iterations,
                inlier_ratio,
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
    pub fn estimate(
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

        let mut rng = RngWrapper::new(self.params.seed);

        // Build sorted index by confidence (descending)
        let mut sorted_indices: Vec<usize> = (0..n).collect();
        sorted_indices.sort_by(|&a, &b| {
            confidences[b]
                .partial_cmp(&confidences[a])
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Compute weights for weighted sampling
        // Higher confidence = higher probability of being sampled
        let weights: Vec<f64> = confidences
            .iter()
            .map(|&c| (c + 0.1).powi(2)) // Square to emphasize high-confidence matches
            .collect();

        self.ransac_loop(
            &ref_points,
            &target_points,
            n,
            min_samples,
            transform_type,
            |iteration, max_iter, sample_buf| {
                // Progressive sampling strategy (3-phase approach):
                // Phase 1 (0-33%): Sample from top 25% high-confidence matches
                // Phase 2 (33-66%): Sample from top 50% matches
                // Phase 3 (66-100%): Uniform random sampling
                let phase = iteration * 3 / max_iter;
                let (pool_size, use_weighted) = match phase {
                    0 => ((n / 4).max(min_samples), true), // Top 25%
                    1 => ((n / 2).max(min_samples), true), // Top 50%
                    _ => (n, false),                       // Full pool
                };

                if use_weighted {
                    weighted_sample_into(
                        &mut rng,
                        &sorted_indices[..pool_size],
                        &weights,
                        min_samples,
                        sample_buf,
                    );
                } else {
                    random_sample_into(&mut rng, n, min_samples, sample_buf);
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
) {
    buffer.clear();

    if pool.len() <= k {
        buffer.extend_from_slice(pool);
        return;
    }

    // Use reservoir sampling with weights (Algorithm A-Res)
    // For each item, compute key = random^(1/weight), keep top k keys
    let mut items_with_keys: Vec<(usize, f64)> = pool
        .iter()
        .map(|&idx| {
            let w = weights.get(idx).copied().unwrap_or(1.0).max(0.001);
            let u: f64 = rng.random();
            let key = u.powf(1.0 / w); // Higher weight = higher expected key
            (idx, key)
        })
        .collect();

    // Partition so the top k elements (by descending key) are in [0..k]
    items_with_keys.select_nth_unstable_by(k - 1, |a, b| {
        b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
    });

    for &(idx, _) in &items_with_keys[..k] {
        buffer.push(idx);
    }
}

/// Randomly sample k unique indices from 0..n into pre-allocated buffer.
///
/// Uses partial Fisher-Yates shuffle: O(k) time, O(n) space.
fn random_sample_into<R: Rng>(rng: &mut R, n: usize, k: usize, buffer: &mut Vec<usize>) {
    debug_assert!(k <= n, "Cannot sample {} indices from {}", k, n);
    buffer.clear();
    buffer.extend(0..n);

    // Partial Fisher-Yates: shuffle only first k elements
    for i in 0..k {
        let j = rng.random_range(i..n);
        buffer.swap(i, j);
    }
    buffer.truncate(k);
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
            if cross.abs() > 1.0 {
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
#[inline]
fn score_hypothesis(
    ref_points: &[DVec2],
    target_points: &[DVec2],
    transform: &Transform,
    scorer: &MagsacScorer,
    inliers: &mut Vec<usize>,
) -> f64 {
    inliers.clear();
    let mut total_loss = 0.0f64;

    for (i, (r, t)) in ref_points.iter().zip(target_points.iter()).enumerate() {
        let p = transform.apply(*r);
        let dist_sq = (p - *t).length_squared();

        total_loss += scorer.loss(dist_sq);
        if scorer.is_inlier(dist_sq) {
            inliers.push(i);
        }
    }

    // Negate so higher score = better model
    -total_loss
}
