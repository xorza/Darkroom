//! [`NormalScores`]: the expected positions of sorted Gaussian samples, for every sample count.

use statrs::distribution::{ContinuousCDF, Normal};

/// Blom's normal scores, `Φ⁻¹((i − 3/8)/(n + 1/4))` for `i = 1..=n`: close approximations to the
/// expected i-th smallest of n unit Gaussian samples (Blom 1958). Sorted Gaussian samples
/// regressed on them lie on the line `μ + σ·z`.
///
/// One flat table for every count, the scores of `n` at `n(n − 1)/2`, filled the first time a pixel
/// has `n` samples. Coverage varies the count from pixel to pixel at the edges of a registered
/// stack, so the scores of one count alone would be computed again at each change.
#[derive(Debug, Default)]
pub(crate) struct NormalScores {
    scores: Vec<f64>,
    filled: Vec<bool>,
}

impl NormalScores {
    pub(crate) fn of_count(&mut self, n: usize) -> &[f64] {
        let start = n * (n - 1) / 2;
        if self.filled.len() <= n {
            self.filled.resize(n + 1, false);
            self.scores.resize(start + n, 0.0);
        }
        let scores = &mut self.scores[start..start + n];
        if !self.filled[n] {
            let normal = Normal::standard();
            let count = n as f64;
            for (rank, score) in scores.iter_mut().enumerate() {
                *score = normal.inverse_cdf((rank as f64 + 1.0 - 0.375) / (count + 0.25));
            }
            self.filled[n] = true;
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
}
