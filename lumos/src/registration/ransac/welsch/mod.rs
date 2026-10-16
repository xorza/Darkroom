//! Robust scoring of a RANSAC hypothesis by a truncated Welsch loss.
//!
//! Instead of a binary inlier/outlier decision, each point gets a continuous loss that grades how
//! well it fits: the Welsch (Leclerc) loss `ρ(r) = (σ²_max/2)·(1 − exp(−r²/(2σ²_max)))`, quadratic
//! (≈ r²/4) near zero and saturating at `σ²_max/2`, held at that value past the χ² 99% boundary. It
//! is monotone non-decreasing in the residual — the property a robust loss must have. MAGSAC++
//! (Barath & Matas 2020) marginalizes its loss over noise scales up to `σ_max`; this takes one
//! scale, `σ_max`, so it is not MAGSAC++.

use crate::math::statistics::CHI2_99_2DOF;

/// Scores a hypothesis's residuals by the truncated Welsch loss of the module docs.
#[derive(Debug)]
pub(super) struct WelschScorer {
    /// `σ²_max`.
    max_sigma_sq: f64,
    /// The loss of a point past the threshold.
    outlier_loss: f64,
    /// The squared outlier threshold, `χ²₀.₉₉(2)·σ²_max`.
    threshold_sq: f64,
}

impl WelschScorer {
    /// The scorer at noise scale `max_sigma`, in pixels: residuals past `√χ²₀.₉₉(2)·max_sigma`,
    /// about `3·max_sigma`, are outliers.
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

        // `1 − e^(−x)` by `expm1`: the subtraction would cancel to nothing for the small `x` of a
        // good fit, where the loss is ≈ r²/4.
        let x = residual_sq / (2.0 * self.max_sigma_sq);
        self.max_sigma_sq / 2.0 * -(-x).exp_m1()
    }

    /// Whether a point counts as an inlier.
    #[inline]
    pub(super) fn is_inlier(&self, residual_sq: f64) -> bool {
        residual_sq <= self.threshold_sq
    }
}

#[cfg(test)]
mod tests;
