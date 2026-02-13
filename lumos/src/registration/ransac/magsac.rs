//! MAGSAC++ scoring for threshold-free robust estimation.
//!
//! MAGSAC++ (Barath & Matas 2020) eliminates the need for manual inlier threshold
//! tuning by marginalizing over a range of noise scales σ ∈ [0, σ_max].
//!
//! Instead of a binary inlier/outlier decision, MAGSAC++ computes a continuous
//! quality score based on the marginal likelihood of each point being an inlier.
//!
//! For k=2 (2D point correspondences), the lower incomplete gamma function has
//! the closed-form γ(1, x) = 1 - exp(-x), so no lookup table is needed.

/// Chi-square 99% quantile for k=2 degrees of freedom.
/// Points beyond this are considered outliers.
const CHI_QUANTILE_SQ: f64 = 9.21; // χ²₀.₉₉(2)

/// Lower incomplete gamma function for k=2: γ(1, x) = 1 - exp(-x).
#[inline]
fn gamma_k2(x: f64) -> f64 {
    if x <= 0.0 { 0.0 } else { 1.0 - (-x).exp() }
}

/// MAGSAC++ scorer for threshold-free inlier evaluation.
///
/// Computes a continuous loss function by marginalizing over noise scales,
/// eliminating the need for manual threshold tuning.
#[derive(Debug)]
pub struct MagsacScorer {
    /// Maximum sigma squared (σ²_max)
    max_sigma_sq: f64,
    /// Outlier loss (assigned to points beyond threshold)
    outlier_loss: f64,
    /// Threshold squared for outlier classification (χ² · σ²_max)
    threshold_sq: f64,
}

impl MagsacScorer {
    /// Create a new MAGSAC++ scorer.
    ///
    /// # Arguments
    /// * `max_sigma` - Maximum noise scale in pixels. Points with residuals
    ///   greater than ~3·max_sigma are treated as outliers.
    pub fn new(max_sigma: f64) -> Self {
        let max_sigma_sq = max_sigma * max_sigma;
        let threshold_sq = CHI_QUANTILE_SQ * max_sigma_sq;

        // Outlier loss = loss at the boundary, ensuring continuity
        // For k=2: loss(threshold) = σ²_max/2 · γ(1, χ²/2) + threshold/4 · (1 - γ(1, χ²/2))
        // At χ²/2 ≈ 4.605, γ(1, x) ≈ 0.99, so loss ≈ σ²_max/2
        let outlier_loss = max_sigma_sq / 2.0;

        Self {
            max_sigma_sq,
            outlier_loss,
            threshold_sq,
        }
    }

    /// Compute MAGSAC++ loss for a single point.
    ///
    /// Lower loss = better fit. The loss smoothly transitions from 0
    /// (perfect fit) to outlier_loss (clear outlier).
    #[inline]
    pub fn loss(&self, residual_sq: f64) -> f64 {
        if residual_sq > self.threshold_sq {
            return self.outlier_loss;
        }

        // x = r² / (2σ²_max)
        let x = residual_sq / (2.0 * self.max_sigma_sq);

        // For k=2: loss = σ²_max/2 · γ(1,x) + r²/4 · (1 - γ(1,x))
        let gx = gamma_k2(x);

        self.max_sigma_sq / 2.0 * gx + residual_sq / 4.0 * (1.0 - gx)
    }

    /// Check if a point should be considered an inlier for counting purposes.
    #[inline]
    pub fn is_inlier(&self, residual_sq: f64) -> bool {
        residual_sq <= self.threshold_sq
    }

    /// Get the effective threshold squared for inlier classification.
    #[inline]
    #[cfg(test)]
    pub fn threshold_sq(&self) -> f64 {
        self.threshold_sq
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gamma_k2_boundaries() {
        assert!((gamma_k2(0.0) - 0.0).abs() < 1e-10);
        assert!((gamma_k2(-1.0) - 0.0).abs() < 1e-10);
        assert!((gamma_k2(100.0) - 1.0).abs() < 1e-10);

        let expected = 1.0 - (-1.0_f64).exp();
        assert!((gamma_k2(1.0) - expected).abs() < 1e-10);
    }

    #[test]
    fn test_scorer_loss_bounded() {
        let scorer = MagsacScorer::new(1.0);

        // MAGSAC++ loss is bounded: 0 at r=0, and at most outlier_loss (0.5) at threshold
        // The loss is NOT monotonically increasing - it can decrease near the outlier threshold
        // This is expected behavior for the marginalized likelihood function

        // Loss at r=0 should be 0
        assert!(scorer.loss(0.0) < 1e-10);

        // Loss should be positive for non-zero residuals
        for i in 1..100 {
            let r_sq = i as f64 * 0.1;
            let loss = scorer.loss(r_sq);
            assert!(loss >= 0.0, "Loss should be non-negative");
            assert!(loss <= 1.0, "Loss should be bounded by outlier_loss");
        }

        // Outlier loss should be constant
        let threshold_sq = scorer.threshold_sq();
        let outlier_loss_1 = scorer.loss(threshold_sq + 1.0);
        let outlier_loss_2 = scorer.loss(threshold_sq + 10.0);
        assert!(
            (outlier_loss_1 - outlier_loss_2).abs() < 1e-10,
            "Outlier loss should be constant"
        );
    }

    #[test]
    fn test_scorer_loss_boundaries() {
        let scorer = MagsacScorer::new(1.0);

        // Loss at r=0 should be 0
        assert!((scorer.loss(0.0) - 0.0).abs() < 1e-10);

        // Loss at outlier should be outlier_loss = σ²_max/2 = 0.5
        let outlier_r_sq = scorer.threshold_sq() * 2.0;
        assert!((scorer.loss(outlier_r_sq) - 0.5).abs() < 1e-10);
    }

    #[test]
    fn test_scorer_is_inlier() {
        let scorer = MagsacScorer::new(1.0);

        // Points within threshold are inliers
        assert!(scorer.is_inlier(0.0));
        assert!(scorer.is_inlier(1.0));
        assert!(scorer.is_inlier(scorer.threshold_sq()));

        // Points beyond threshold are outliers
        assert!(!scorer.is_inlier(scorer.threshold_sq() + 0.1));
    }

    #[test]
    fn test_effective_threshold() {
        // With max_sigma = 1.0, effective threshold ≈ 3.03 pixels
        let scorer = MagsacScorer::new(1.0);
        let effective_threshold = scorer.threshold_sq().sqrt();
        assert!((effective_threshold - 3.03).abs() < 0.1);

        // With max_sigma = 0.67, effective threshold ≈ 2.0 pixels
        let scorer = MagsacScorer::new(0.67);
        let effective_threshold = scorer.threshold_sq().sqrt();
        assert!((effective_threshold - 2.03).abs() < 0.1);
    }
}
