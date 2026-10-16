//! [`RingingClamp`]: PixInsight's limit on what a kernel's negative lobes may take from a sample.

/// One channel's sums over a window, split as the clamp reads them.
///
/// The clamp is defined on light, values at or above zero, so it reads `f⁺ = max(f, 0)` by the
/// sign of each tap's weight `L`; what lies below zero passes through the plain normalized kernel.
/// For data that never goes below zero `below_zero` is zero and the clamp is PixInsight's exactly.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct LobeSums {
    /// `Σ L·f⁺` over the taps with `L > 0`.
    pub(crate) positive: f32,
    /// `Σ L·f⁺` over the taps with `L < 0`: zero or less.
    pub(crate) negative: f32,
    /// `Σ L·min(f, 0)` over every tap.
    pub(crate) below_zero: f32,
}

/// A window's weight split by lobe: `Σ L` over the taps with `L > 0`, and over those with `L < 0`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LobeWeights {
    pub(crate) positive: f32,
    pub(crate) negative: f32,
}

/// The ringing clamp of PixInsight's Lanczos interpolation (PCL `LanczosInterpolation`).
///
/// A kernel with negative lobes undershoots beside a bright source: Lanczos3 reads a single bright
/// pixel under its first negative lobe at about −13% of its value. The clamp measures the share
/// `r = −negative / positive` the negative lobes take from what the positive lobes carry. Up to
/// the threshold the sample is the plain kernel's. Past it, the negative lobes' sum and weight are
/// both scaled by `1 − ((r − t)/(1 − t))²`, which falls smoothly from 1 at `r = t` to 0 at
/// `r = 1`, where the sample would otherwise reach zero; from there on the sample is the
/// positive lobes' weighted mean. A flat field is untouched at every `r`, since its sums scale
/// with its weights. PCL splits the taps by the sign of `L·f`; here they split by the sign of `L`,
/// the same partition for data above zero, and one that keeps every weight sum the sign it names
/// where data dips below.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RingingClamp {
    threshold: f32,
}

impl RingingClamp {
    pub(crate) const fn new(threshold: ClampingThreshold) -> Self {
        Self {
            threshold: threshold.get(),
        }
    }

    /// The clamped sample from a window's `sums`, the `total` weight of its taps that hold data,
    /// which is positive, and their weight by lobe, taken only where the clamp acts.
    ///
    /// With `wₚ` and `wₙ` the two lobes' weights, the light's part is `(sₚ + c·sₙ)/(wₚ + c·wₙ)`
    /// for the factor `c` above, whose denominator is at least `total`; the part below zero is
    /// `below_zero / total`.
    #[inline(always)]
    pub(crate) fn sample(
        self,
        sums: LobeSums,
        total: f32,
        lobes: impl FnOnce() -> LobeWeights,
    ) -> f32 {
        sums.below_zero / total + self.light(sums, total, lobes)
    }

    #[inline(always)]
    fn light(self, sums: LobeSums, total: f32, lobes: impl FnOnce() -> LobeWeights) -> f32 {
        if sums.positive == 0.0 {
            return 0.0;
        }
        // The ratio `−negative / positive` against 1 and the threshold, as products: the positive
        // sum is above zero here, and most samples stop at the second test.
        let undershoot = -sums.negative;
        if undershoot < sums.positive && undershoot <= self.threshold * sums.positive {
            return (sums.positive + sums.negative) / total;
        }
        let weights = lobes();
        if undershoot >= sums.positive {
            return sums.positive / weights.positive;
        }
        // A threshold of 1 never gets here: the ratio is above it and below 1.
        let ratio = undershoot / sums.positive;
        let excess = (ratio - self.threshold) / (1.0 - self.threshold);
        let keep = 1.0 - excess * excess;
        (sums.positive + keep * sums.negative) / (weights.positive + keep * weights.negative)
    }
}

#[cfg(test)]
mod tests;
use crate::registration::resample::warp_params::ClampingThreshold;
