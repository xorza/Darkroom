//! Robust scoring of a RANSAC hypothesis by a truncated Welsch loss.
//!
//! Instead of a binary inlier/outlier decision, each point gets a continuous loss that grades how
//! well it fits: the Welsch (Leclerc) loss `ρ(r) = (σ²_max/2)·(1 − exp(−r²/(2σ²_max)))`, quadratic
//! (≈ r²/4) near zero and saturating at `σ²_max/2`, held at that value past the χ² 99% boundary. It
//! is monotone non-decreasing in the residual — the property a robust loss must have. MAGSAC++
//! (Barath & Matas 2020) marginalizes its loss over noise scales up to `σ_max`; this takes one
//! scale, `σ_max`, so it is not MAGSAC++.

use crate::math::statistics::CHI2_99_2DOF;

/// Lower incomplete gamma function for k=2: γ(1, x) = 1 - exp(-x).
#[inline]
fn gamma_k2(x: f64) -> f64 {
    if x <= 0.0 { 0.0 } else { 1.0 - (-x).exp() }
}

/// Scores a hypothesis's residuals by the truncated Welsch loss of the module docs.
#[derive(Debug)]
pub(super) struct WelschScorer {
    /// Maximum sigma squared (`σ²_max`)
    max_sigma_sq: f64,
    /// Outlier loss (assigned to points beyond threshold)
    outlier_loss: f64,
    /// Threshold squared for outlier classification (χ² · `σ²_max`)
    threshold_sq: f64,
}

impl WelschScorer {
    /// The scorer at noise scale `max_sigma`.
    ///
    /// # Arguments
    /// * `max_sigma` - Maximum noise scale in pixels. Points with residuals
    ///   greater than ~`3·max_sigma` are treated as outliers.
    pub(super) fn new(max_sigma: f64) -> Self {
        let max_sigma_sq = max_sigma * max_sigma;
        let threshold_sq = CHI2_99_2DOF * max_sigma_sq;

        // An outlier costs the loss's saturation value, σ²_max/2. At the threshold the loss has
        // reached γ(1, χ²/2) = 1 − e^(−4.605) = 0.99 of it, so the step to an outlier is up by
        // 1%: the loss stays monotone.
        let outlier_loss = max_sigma_sq / 2.0;

        Self {
            max_sigma_sq,
            outlier_loss,
            threshold_sq,
        }
    }

    /// The loss of a single point.
    ///
    /// Lower loss = better fit. The loss smoothly transitions from 0
    /// (perfect fit) to `outlier_loss` (clear outlier).
    #[inline]
    pub(super) fn loss(&self, residual_sq: f64) -> f64 {
        if residual_sq > self.threshold_sq {
            return self.outlier_loss;
        }

        // x = r² / (2σ²_max)
        let x = residual_sq / (2.0 * self.max_sigma_sq);

        // Monotone saturating loss: ≈ r²/4 near zero (least-squares), saturating at σ²_max/2 as the
        // residual grows. (An earlier `+ r²/4·(1−γ)` term made the loss climb past the outlier
        // value around r≈2σ then fall back — a non-monotone shape a robust loss must not have.)
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
