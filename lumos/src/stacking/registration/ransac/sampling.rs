//! Choosing which correspondences a hypothesis is built from.
//!
//! Uniform random sampling wastes iterations on pairs the triangle vote already found
//! unconvincing. Two guided phases come first — the top quarter by confidence, then the top half,
//! each sampled weighted by confidence — so a good hypothesis is usually found early, and sampling
//! is uniform over every pair after them.
//!
//! The guided phases have a fixed length, and the adaptive stop counts only the uniform iterations
//! after them. The stop's guarantee — an all-inlier sample drawn with the configured confidence —
//! is a statement about uniform sampling of the whole set. Counted over guided iterations it would
//! end a run whose samples never left the top quarter, and a larger consensus among the other pairs
//! would never be drawn.

use std::cmp::Ordering;

use rand::prelude::*;

use crate::stacking::registration::ransac::transforms::adaptive_iterations;

/// The share of the pairs, by confidence, each guided phase samples from: the top quarter, then
/// the top half.
pub(super) const GUIDED_POOL_FRACTIONS: [f64; 2] = [0.25, 0.50];

/// Iterations in each guided phase: enough to draw an all-inlier sample of `sample_size` with
/// `confidence` from a pool at least half inliers — the pools a guided phase is worth its cost on.
pub(super) fn guided_phase_iterations(sample_size: usize, confidence: f64) -> usize {
    adaptive_iterations(0.5, sample_size, confidence)
}

/// Create a `ChaCha8Rng` from an optional seed.
///
/// When `seed` is `None`, seeds from `thread_rng()` for non-deterministic behavior.
/// Always using `ChaCha8Rng` avoids enum dispatch overhead on every RNG call.
pub(super) fn make_rng(seed: Option<u64>) -> rand_chacha::ChaCha8Rng {
    match seed {
        Some(s) => rand_chacha::ChaCha8Rng::seed_from_u64(s),
        None => rand_chacha::ChaCha8Rng::seed_from_u64(rand::rng().next_u64()),
    }
}

/// Weighted sampling of k unique indices from a pool.
///
/// Samples indices with probability proportional to their weights using
/// Algorithm A-Res (reservoir sampling with weights). Uses `select_nth_unstable`
/// for O(n) average-case partitioning instead of a full O(n log n) sort.
pub(super) fn weighted_sample_into<R: Rng>(
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
        // `weights` has one entry per point and `idx` indexes the same `0..n` pool, so it can't
        // miss.
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
pub(super) fn random_sample_into<R: Rng>(
    rng: &mut R,
    n: usize,
    k: usize,
    buffer: &mut Vec<usize>,
    indices: &mut Vec<usize>,
) {
    debug_assert!(k <= n, "Cannot sample {k} indices from {n}");

    // Initialize or resize the persistent index array
    if indices.len() != n {
        indices.clear();
        indices.extend(0..n);
    }

    // Partial Fisher-Yates: shuffle first k elements, recording swap targets
    buffer.clear();
    // k is a minimal sample, at most a homography's 4, so a stack array suffices.
    let mut swap_targets = [0usize; 8];
    debug_assert!(
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
