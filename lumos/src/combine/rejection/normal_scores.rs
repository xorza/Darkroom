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

    pub(crate) fn of_count(&mut self, n: usize) -> &[f64] {
        debug_assert!(n > 0, "the scores of no samples");
        self.reserve(n);
        if self.counts.is_empty() {
            let rows = (CACHE_BYTES / (self.max_count * size_of::<f64>())).clamp(1, self.max_count);
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
mod tests {
    use super::*;

    /// `Φ⁻¹(1.625/2.25)` from `scipy.stats.norm.ppf`.
    const SCIPY_TWO: f64 = 0.589_455_797_849_778_3;

    /// One sample sits at the median: Φ⁻¹(0.625/1.25) = Φ⁻¹(0.5) = 0. Two sit at
    /// ±Φ⁻¹(1.625/2.25), which SciPy gives as 0.5894558; statrs computes Φ⁻¹ through erfc⁻¹ with a
    /// relative error near 1e-15, so 1e-12 holds the value and the odd symmetry about the middle. A
    /// smaller count filled after a larger one finds its own slice.
    #[test]
    fn scores_are_blom_positions_for_each_count() {
        let mut scores = NormalScores::default();
        let five = scores.of_count(5).to_vec();
        assert_eq!(scores.of_count(1), [0.0]);
        let two = scores.of_count(2).to_vec();
        assert!((two[1] - SCIPY_TWO).abs() < 1e-12, "{two:?}");
        assert!((two[0] + two[1]).abs() < 1e-12, "{two:?}");
        for rank in 0..5 {
            assert!((five[rank] + five[4 - rank]).abs() < 1e-12, "{five:?}");
        }
        assert_eq!(scores.of_count(5), five);
    }

    /// The cache holds a row per count up to 362 frames: counts 1 to 362 fall in distinct rows of
    /// 362, and 362 rows of 362 scores are 1 048 352 B, within 1 MiB. At 5000 frames it holds
    /// ⌊1 048 576 / 40 000⌋ = 26 rows, so counts 5000 and 8 share row 8 and refill it, and each
    /// still finds its own scores.
    #[test]
    fn the_cache_stays_within_its_bytes_and_keeps_every_count() {
        let mut small = NormalScores::default();
        small.reserve(362);
        assert!(
            small.counts.is_empty(),
            "no row before a score is asked for"
        );
        small.of_count(3);
        assert_eq!(small.counts.len(), 362);
        let mut large = NormalScores::default();
        large.reserve(5000);
        large.of_count(1);
        assert_eq!(large.counts.len(), 26);
        assert!(large.scores.len() * size_of::<f64>() <= CACHE_BYTES);
        let full = large.of_count(5000).to_vec();
        let few = large.of_count(8).to_vec();
        assert_eq!(large.counts[8], 8);
        assert_eq!(large.of_count(5000), full);
        assert_eq!(large.counts[8], 5000);
        assert_eq!(large.of_count(8), few);
        assert_eq!(few, NormalScores::default().of_count(8));
        assert_eq!(full, NormalScores::default().of_count(5000));
    }
}
