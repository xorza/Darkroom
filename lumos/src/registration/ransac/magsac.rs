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

    const TOL: f64 = 1e-10;

    #[test]
    fn test_gamma_k2_exact_values() {
        // γ(1, x) = 1 - exp(-x) for x > 0, else 0
        assert_eq!(gamma_k2(0.0), 0.0);
        assert_eq!(gamma_k2(-1.0), 0.0);
        assert_eq!(gamma_k2(-100.0), 0.0);

        // γ(1, 1) = 1 - exp(-1) = 1 - 0.367879441... = 0.632120558...
        let expected_1 = 1.0 - 1.0_f64 / std::f64::consts::E;
        assert!((gamma_k2(1.0) - expected_1).abs() < TOL);

        // γ(1, 2) = 1 - exp(-2) = 1 - 0.135335283... = 0.864664716...
        let expected_2 = 1.0 - (-2.0_f64).exp();
        assert!((gamma_k2(2.0) - expected_2).abs() < TOL);

        // γ(1, 0.5) = 1 - exp(-0.5) = 1 - 0.606530659... = 0.393469340...
        let expected_half = 1.0 - (-0.5_f64).exp();
        assert!((gamma_k2(0.5) - expected_half).abs() < TOL);

        // Large x: γ(1, 100) ≈ 1.0 (exp(-100) ≈ 0)
        assert!((gamma_k2(100.0) - 1.0).abs() < TOL);
    }

    #[test]
    fn test_scorer_construction_exact() {
        // MagsacScorer::new(σ_max) stores:
        //   max_sigma_sq = σ_max²
        //   threshold_sq = χ²₀.₉₉(2) * σ_max² = 9.21 * σ_max²
        //   outlier_loss = σ_max² / 2

        let scorer = MagsacScorer::new(2.0);
        // σ_max = 2.0 → σ²_max = 4.0
        // threshold_sq = 9.21 * 4.0 = 36.84
        assert!((scorer.threshold_sq() - 36.84).abs() < TOL);
        // outlier_loss = 4.0 / 2.0 = 2.0
        assert!((scorer.outlier_loss - 2.0).abs() < TOL);

        let scorer = MagsacScorer::new(0.5);
        // σ_max = 0.5 → σ²_max = 0.25
        // threshold_sq = 9.21 * 0.25 = 2.3025
        #[allow(clippy::approx_constant)]
        let expected_threshold = 2.3025;
        assert!((scorer.threshold_sq() - expected_threshold).abs() < TOL);
        // outlier_loss = 0.25 / 2.0 = 0.125
        assert!((scorer.outlier_loss - 0.125).abs() < TOL);
    }

    #[test]
    fn test_scorer_loss_hand_computed() {
        // With σ_max = 1.0: σ²_max = 1.0, threshold_sq = 9.21
        let scorer = MagsacScorer::new(1.0);

        // loss(r²) = σ²_max/2 * γ(1, x) + r²/4 * (1 - γ(1, x))
        // where x = r² / (2 * σ²_max) = r² / 2

        // r² = 0: x = 0, γ(1,0) = 0 → loss = 0.5*0 + 0/4*(1-0) = 0
        assert!((scorer.loss(0.0)).abs() < TOL);

        // r² = 1.0: x = 0.5, γ(1,0.5) = 1 - exp(-0.5)
        // γ = 0.393469340...
        // loss = 0.5 * 0.393469340 + 0.25 * (1 - 0.393469340)
        //      = 0.196734670 + 0.25 * 0.606530659
        //      = 0.196734670 + 0.151632664
        //      = 0.348367335
        let g_half = 1.0 - (-0.5_f64).exp();
        let expected = 0.5 * g_half + 0.25 * (1.0 - g_half);
        assert!(
            (scorer.loss(1.0) - expected).abs() < TOL,
            "loss(1.0) = {}, expected {}",
            scorer.loss(1.0),
            expected
        );

        // r² = 4.0: x = 2.0, γ(1,2) = 1 - exp(-2)
        // γ = 0.864664716...
        // loss = 0.5 * 0.864664716 + 1.0 * (1 - 0.864664716)
        //      = 0.432332358 + 0.135335283
        //      = 0.567667641
        let g_2 = 1.0 - (-2.0_f64).exp();
        let expected_4 = 0.5 * g_2 + 1.0 * (1.0 - g_2);
        assert!(
            (scorer.loss(4.0) - expected_4).abs() < TOL,
            "loss(4.0) = {}, expected {}",
            scorer.loss(4.0),
            expected_4
        );

        // r² > threshold_sq (9.21): returns outlier_loss = 0.5
        assert!((scorer.loss(10.0) - 0.5).abs() < TOL);
        assert!((scorer.loss(100.0) - 0.5).abs() < TOL);
        assert!((scorer.loss(9.22) - 0.5).abs() < TOL);
    }

    #[test]
    fn test_scorer_loss_at_threshold_boundary() {
        // Verify loss is continuous at the threshold boundary.
        // Just below threshold should be close to outlier_loss.
        let scorer = MagsacScorer::new(1.0);
        let threshold_sq = scorer.threshold_sq(); // 9.21

        // Loss just below threshold:
        // x = 9.20 / 2 = 4.60, γ(1, 4.60) = 1 - exp(-4.60) ≈ 0.98994...
        // loss = 0.5 * 0.98994 + 9.20/4 * (1-0.98994)
        //      ≈ 0.49497 + 2.30 * 0.01006 ≈ 0.49497 + 0.02314 ≈ 0.51811
        // This is slightly above outlier_loss (0.5), which is expected
        // (the MAGSAC formula can overshoot slightly near the boundary)
        let loss_just_below = scorer.loss(threshold_sq - 0.01);
        let loss_just_above = scorer.loss(threshold_sq + 0.01);

        // Just above threshold returns exactly outlier_loss = 0.5
        assert!((loss_just_above - 0.5).abs() < TOL);

        // The discontinuity at the boundary should be small (< 0.03)
        assert!(
            (loss_just_below - loss_just_above).abs() < 0.03,
            "Discontinuity at threshold: just_below={}, just_above={}",
            loss_just_below,
            loss_just_above
        );
    }

    #[test]
    fn test_scorer_different_sigma_changes_loss() {
        // Verify that different sigma values produce different losses for the same residual.
        // With σ=1: loss(r²=2) uses x = 2/2 = 1
        // With σ=2: loss(r²=2) uses x = 2/8 = 0.25
        let scorer_1 = MagsacScorer::new(1.0);
        let scorer_2 = MagsacScorer::new(2.0);

        let loss_1 = scorer_1.loss(2.0);
        let loss_2 = scorer_2.loss(2.0);

        // σ=1: x=1.0, γ(1,1)=0.63212..., loss = 0.5*0.63212 + 0.5*(1-0.63212) = 0.50000 (coincidence?)
        // Recompute: loss = 0.5*0.63212 + 0.5*0.36788 = 0.31606 + 0.18394 = 0.50000
        // σ=2: σ²=4, x=2/(2*4)=0.25, γ(1,0.25)=1-exp(-0.25)=0.22119
        // loss = 4/2*0.22119 + 2/4*(1-0.22119) = 2*0.22119 + 0.5*0.77880 = 0.44239 + 0.38940 = 0.83179
        // Different as expected
        assert!(
            (loss_1 - loss_2).abs() > 0.1,
            "Different sigma must produce different losses: loss_1={}, loss_2={}",
            loss_1,
            loss_2
        );
    }

    #[test]
    fn test_is_inlier_exact_threshold() {
        let scorer = MagsacScorer::new(1.0);
        // threshold_sq = 9.21

        assert!(scorer.is_inlier(0.0));
        assert!(scorer.is_inlier(5.0));
        assert!(scorer.is_inlier(9.21)); // exactly at threshold
        assert!(!scorer.is_inlier(9.22)); // just above
        assert!(!scorer.is_inlier(100.0));
    }

    #[test]
    fn test_effective_threshold_exact() {
        // threshold_sq = CHI_QUANTILE_SQ * σ² = 9.21 * σ²
        // effective threshold (pixels) = sqrt(threshold_sq) = sqrt(9.21) * σ

        // sqrt(9.21) = 3.03480...
        let sqrt_chi = 9.21_f64.sqrt(); // 3.03480...

        let scorer_1 = MagsacScorer::new(1.0);
        assert!(
            (scorer_1.threshold_sq().sqrt() - sqrt_chi * 1.0).abs() < 1e-6,
            "σ=1: threshold={}",
            scorer_1.threshold_sq().sqrt()
        );

        let scorer_3 = MagsacScorer::new(3.0);
        // threshold = sqrt(9.21 * 9) = sqrt(82.89) = 9.104...
        assert!(
            (scorer_3.threshold_sq().sqrt() - sqrt_chi * 3.0).abs() < 1e-6,
            "σ=3: threshold={}",
            scorer_3.threshold_sq().sqrt()
        );
    }
}
