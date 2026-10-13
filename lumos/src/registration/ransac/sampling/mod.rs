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

use rand::prelude::*;
use rand_chacha::ChaCha8Rng;

use crate::registration::ransac::ransac_config::RansacConfig;
use crate::registration::ransac::transforms::adaptive_iterations;

/// The share of the pairs, by confidence, each guided phase samples from: the top quarter, then
/// the top half.
const GUIDED_POOL_FRACTIONS: [f64; 2] = [0.25, 0.50];

/// A minimal sample's largest size: a homography's four points, with room to spare.
const MAX_SAMPLE_SIZE: usize = 8;

/// Draws one estimation's minimal samples: the guided phases first, then uniform ones.
#[derive(Debug)]
pub(super) struct Sampler {
    rng: ChaCha8Rng,
    sample_size: usize,
    /// Iterations in each guided phase.
    phase_length: usize,
    /// The pairs by descending confidence, ties in index order.
    by_confidence: Vec<usize>,
    /// Each pair's sampling weight, `(confidence + 0.1)²`, which favours the confident ones.
    weights: Vec<f64>,
    /// `0..n` between draws: a uniform draw shuffles its head and undoes the swaps after.
    shuffle: Vec<usize>,
    /// Each pool member's A-Res key, reused across the weighted draws.
    keys: Vec<(usize, f64)>,
}

impl Sampler {
    /// The sampler of minimal samples of `sample_size` over pairs at `confidences`, seeded and
    /// phased as `config` sets.
    pub(super) fn new(confidences: &[f64], sample_size: usize, config: &RansacConfig) -> Self {
        debug_assert!(sample_size <= MAX_SAMPLE_SIZE);
        let mut by_confidence: Vec<usize> = (0..confidences.len()).collect();
        by_confidence.sort_by(|&a, &b| confidences[b].total_cmp(&confidences[a]));
        Self {
            rng: ChaCha8Rng::seed_from_u64(config.seed),
            sample_size,
            phase_length: guided_phase_iterations(sample_size, config.confidence),
            by_confidence,
            weights: confidences.iter().map(|&c| (c + 0.1).powi(2)).collect(),
            shuffle: Vec::new(),
            keys: Vec::new(),
        }
    }

    /// The iterations the guided phases take, which the adaptive stop does not count.
    pub(super) const fn guided_iterations(&self) -> usize {
        GUIDED_POOL_FRACTIONS.len() * self.phase_length
    }

    /// The sample of `iteration`, numbered from 1, into `sample`.
    #[expect(
        clippy::cast_sign_loss,
        reason = "the sampling fraction of a pair count is non-negative"
    )]
    pub(super) fn draw(&mut self, iteration: usize, sample: &mut Vec<usize>) {
        match GUIDED_POOL_FRACTIONS.get((iteration - 1) / self.phase_length) {
            Some(&fraction) => {
                let n = self.by_confidence.len();
                let pool_size = ((n as f64 * fraction).ceil() as usize).max(self.sample_size);
                self.weighted(pool_size, sample);
            }
            None => self.uniform(sample),
        }
    }

    /// A weighted sample of distinct pairs from the `pool_size` most confident, by Algorithm A-Res
    /// (Efraimidis and Spirakis 2006): each pair draws the key `u^(1/w)` and the largest keys win.
    /// `select_nth_unstable` partitions them out in O(n) rather than sorting. A pool no larger
    /// than a sample is taken whole.
    fn weighted(&mut self, pool_size: usize, sample: &mut Vec<usize>) {
        let k = self.sample_size;
        let pool = &self.by_confidence[..pool_size];
        sample.clear();
        if pool.len() <= k {
            sample.extend_from_slice(pool);
            return;
        }
        self.keys.clear();
        self.keys.extend(pool.iter().map(|&index| {
            let weight = self.weights[index].max(0.001);
            let u: f64 = self.rng.random();
            (index, u.powf(1.0 / weight))
        }));
        self.keys
            .select_nth_unstable_by(k - 1, |a, b| b.1.total_cmp(&a.1));
        sample.extend(self.keys[..k].iter().map(|&(index, _)| index));
    }

    /// A uniform sample of distinct pairs by a partial Fisher-Yates shuffle, O(k): the head of
    /// `shuffle` is shuffled, read, and its swaps undone, so `shuffle` is `0..n` again for the next
    /// draw without being rebuilt.
    fn uniform(&mut self, sample: &mut Vec<usize>) {
        let n = self.by_confidence.len();
        let k = self.sample_size;
        debug_assert!(k <= n, "Cannot sample {k} indices from {n}");
        if self.shuffle.len() != n {
            self.shuffle.clear();
            self.shuffle.extend(0..n);
        }
        sample.clear();
        let mut swap_targets = [0usize; MAX_SAMPLE_SIZE];
        for (i, target) in swap_targets[..k].iter_mut().enumerate() {
            let j = self.rng.random_range(i..n);
            self.shuffle.swap(i, j);
            *target = j;
            sample.push(self.shuffle[i]);
        }
        for (i, &target) in swap_targets[..k].iter().enumerate().rev() {
            self.shuffle.swap(i, target);
        }
    }
}

/// Iterations in each guided phase: enough to draw an all-inlier sample of `sample_size` with
/// `confidence` from a pool at least half inliers — the pools a guided phase is worth its cost on.
fn guided_phase_iterations(sample_size: usize, confidence: f64) -> usize {
    adaptive_iterations(0.5, sample_size, confidence)
}

#[cfg(test)]
mod tests;
