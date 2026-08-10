//! Choosing which correspondences a hypothesis is built from.
//!
//! Uniform random sampling wastes iterations on pairs the triangle vote already found
//! unconvincing. Sampling ramps through three phases instead — the top quarter by confidence,
//! then the top half, then everything — weighted for the first two, so good candidates are
//! front-loaded without ever excluding a pair the later phases might need.

use std::cmp::Ordering;

use rand::prelude::*;

/// Progressive sampling: 3 phases from high-confidence pool to full pool.
/// This front-loads good candidates, improving early convergence.
pub(super) const SAMPLING_PHASES: usize = 3;
/// Pool fraction per phase: top 25% → top 50% → full pool.
pub(super) const PHASE_POOL_FRACTIONS: [f64; 3] = [0.25, 0.50, 1.0];
/// Whether each phase uses weighted sampling (vs uniform random).
pub(super) const PHASE_WEIGHTED: [bool; 3] = [true, true, false];

/// Create a ChaCha8Rng from an optional seed.
///
/// When `seed` is `None`, seeds from `thread_rng()` for non-deterministic behavior.
/// Always using ChaCha8Rng avoids enum dispatch overhead on every RNG call.
pub(super) fn make_rng(seed: Option<u64>) -> rand_chacha::ChaCha8Rng {
    use rand_chacha::rand_core::SeedableRng;
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
pub(super) fn random_sample_into<R: Rng>(
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
