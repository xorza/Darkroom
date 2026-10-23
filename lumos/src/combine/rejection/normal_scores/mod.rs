//! [`NormalScores`]: the expected positions of sorted Gaussian samples, for every sample count.

use statrs::distribution::{ContinuousCDF, Normal};

/// The bytes a [`NormalScores`] cache holds at most, once its rows reach one count each: a worker's
/// share of the scratch the memory plan leaves outside its budget.
const CACHE_BYTES: usize = 1 << 20;

/// Blom's normal scores, `Φ⁻¹((i − 3/8)/(n + 1/4))` for `i = 1..=n`: close approximations to the
/// expected i-th smallest of n unit Gaussian samples (Blom 1958). Sorted Gaussian samples
/// regressed on them lie on the line `μ + σ·z`.
///
/// A cache of rows indexed by count: the scores of `n` are in row `n mod rows`, filled when a pixel
/// with `n` samples finds another count there. Coverage changes the count only at the edges of a
/// registered stack, and between neighbouring counts, which fall in neighbouring rows, so a row is
/// seldom filled twice. Up to 362 frames every count has its own row. Above that the cache stays at
/// [`CACHE_BYTES`] rather than a table of every count, which at 5000 frames is 100 MB per worker.
/// A refilled row holds the same scores, so the cache size never changes a result.
#[derive(Debug, Default)]
pub(crate) struct NormalScores {
    /// `counts.len()` rows of `max_count` scores.
    scores: Vec<f64>,
    /// The count each row holds the scores of, `0` for none.
    counts: Vec<usize>,
    max_count: usize,
}

impl NormalScores {
    /// Size the cache for counts up to `max_count`, so no count a pixel brings regrows it. The
    /// rows are allocated by the first pixel that needs a score: a method that fits no line holds
    /// none.
    pub(crate) fn reserve(&mut self, max_count: usize) {
        if max_count > self.max_count {
            self.max_count = max_count;
            self.counts.clear();
            self.scores.clear();
        }
    }

    /// The bytes the cache holds at most for counts up to `max_count`: its rows of scores and
    /// their counts; none for no count.
    pub(crate) const fn heap_bytes(max_count: usize) -> usize {
        if max_count == 0 {
            return 0;
        }
        let rows = Self::rows(max_count);
        rows * max_count * size_of::<f64>() + rows * size_of::<usize>()
    }

    /// The rows a cache for counts up to `max_count` keeps: as many as fit [`CACHE_BYTES`], at
    /// least one and at most a row for each count.
    const fn rows(max_count: usize) -> usize {
        let fit = CACHE_BYTES / (max_count * size_of::<f64>());
        if fit > max_count {
            max_count
        } else if fit == 0 {
            1
        } else {
            fit
        }
    }

    pub(super) fn of_count(&mut self, n: usize) -> &[f64] {
        debug_assert!(n > 0, "the scores of no samples");
        self.reserve(n);
        if self.counts.is_empty() {
            let rows = Self::rows(self.max_count);
            self.counts.resize(rows, 0);
            self.scores.resize(rows * self.max_count, 0.0);
        }
        let row = n % self.counts.len();
        let scores = &mut self.scores[row * self.max_count..][..n];
        if self.counts[row] != n {
            let normal = Normal::standard();
            let count = n as f64;
            for (rank, score) in scores.iter_mut().enumerate() {
                *score = normal.inverse_cdf((rank as f64 + 1.0 - 0.375) / (count + 0.25));
            }
            self.counts[row] = n;
        }
        scores
    }
}

#[cfg(test)]
mod tests;
