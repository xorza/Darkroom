//! The DAOFIND roundness metrics.

use serde::{Deserialize, Serialize};

/// The pair of DAOFIND roundness metrics measured for one source.
///
/// Both are zero for a circular, symmetric source, and they catch different departures from it:
/// GROUND sees elongation along an axis, SROUND sees a lopsided profile. A star has to satisfy
/// both, which is why they travel together.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Roundness {
    /// GROUND, from the marginal distributions: `(Hx - Hy) / (Hx + Hy)`, where `Hx` and `Hy`
    /// are the peak heights of the x and y marginals. Circular → 0, x-extended → negative,
    /// y-extended → positive.
    pub ground: f32,
    /// SROUND, from bilateral symmetry: the RMS of the marginals' left/right and top/bottom
    /// imbalance. Circular → 0, asymmetric → positive.
    pub sround: f32,
}

impl Roundness {
    /// Measure both metrics from a stamp's marginal distributions.
    pub(crate) fn from_marginals(marginal_x: &[f64], marginal_y: &[f64]) -> Self {
        let hx = marginal_x.iter().copied().fold(0.0f64, f64::max);
        let hy = marginal_y.iter().copied().fold(0.0f64, f64::max);
        let ground = safe_ratio(hx - hy, hx + hy);

        let center = marginal_x.len() / 2;
        let asym_x = asymmetry(marginal_x, center);
        let asym_y = asymmetry(marginal_y, center);

        Self {
            ground: (ground as f32).clamp(-1.0, 1.0),
            sround: (asym_x.hypot(asym_y) as f32).clamp(0.0, 1.0),
        }
    }
}

/// `(after − before) / (after + before)` over the halves of `slice` either side of `center`,
/// which is excluded.
#[inline]
fn asymmetry(slice: &[f64], center: usize) -> f64 {
    let before: f64 = slice[..center].iter().sum();
    let after: f64 = slice[center + 1..].iter().sum();
    safe_ratio(after - before, before + after)
}

/// Safe division returning 0.0 when denominator is near zero.
#[inline]
fn safe_ratio(numerator: f64, denominator: f64) -> f64 {
    if denominator > f64::EPSILON {
        numerator / denominator
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests;
