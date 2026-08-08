//! MAGSAC++-inspired scoring for threshold-free robust estimation.
//!
//! Inspired by MAGSAC++ (Barath & Matas 2020): instead of a binary inlier/outlier decision, each
//! point gets a continuous loss that grades how well it fits, removing manual threshold tuning.
//!
//! This is **not** the paper's exact `ρ` (which uses the n=4 DoF incomplete gammas). It is a
//! lighter monotone saturating loss built on the closed-form `γ(1, x) = 1 − exp(−x)` (no lookup
//! table): quadratic (≈ r²/4) near zero, saturating at `σ²_max/2`. It is monotone non-decreasing
//! in the residual — the property a robust loss must have.

/// Chi-square 99% quantile for k=2 degrees of freedom.
/// Points beyond this are considered outliers.
const CHI_QUANTILE_SQ: f64 = 9.21; // χ²₀.₉₉(2)

/// Lower incomplete gamma function for k=2: γ(1, x) = 1 - exp(-x).
#[inline]
fn gamma_k2(x: f64) -> f64 {
    if x <= 0.0 { 0.0 } else { 1.0 - (-x).exp() }
}

/// MAGSAC++-inspired scorer for threshold-free inlier evaluation.
///
/// Computes a continuous, monotone saturating loss instead of a binary inlier decision (see the
/// module docs — this is a lighter loss than the paper's exact `ρ`).
#[derive(Debug)]
pub(super) struct MagsacScorer {
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
    pub(super) fn new(max_sigma: f64) -> Self {
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
    pub(super) fn loss(&self, residual_sq: f64) -> f64 {
        if residual_sq > self.threshold_sq {
            return self.outlier_loss;
        }

        // x = r² / (2σ²_max)
        let x = residual_sq / (2.0 * self.max_sigma_sq);

        // Monotone saturating loss: ≈ r²/4 near zero (least-squares), saturating at σ²_max/2 as the
        // residual grows. (An earlier `+ r²/4·(1−γ)` term made the loss climb past the outlier value
        // around r≈2σ then fall back — a non-monotone shape a robust loss must not have.)
        self.max_sigma_sq / 2.0 * gamma_k2(x)
    }

    /// Check if a point should be considered an inlier for counting purposes.
    #[inline]
    pub(super) fn is_inlier(&self, residual_sq: f64) -> bool {
        residual_sq <= self.threshold_sq
    }
}

#[cfg(test)]
mod tests;
