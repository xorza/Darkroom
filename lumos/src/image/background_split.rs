//! [`BackgroundSplit`]: a frame's background noise split by how a flat amplified it.

use serde::{Deserialize, Serialize};

/// The background variance of a flat-divided frame: `σ²·(ρ·g² + (1 − ρ)·g)` at a pixel the flat
/// multiplied by `g = 1/f`.
///
/// The read noise and the dark current were added before the flat shaped the light, so the
/// division scales their variance `A` by `g²`. The sky's photons arrived through the vignetting:
/// their count, and with it their variance, is `f` times the unvignetted one, which the division
/// leaves at `S·g`. That is the CCD equation carried through a flat, as ccdproc propagates it. Most
/// frames state no gain to separate the two by, so the frame's own noise, measured over bins of
/// `g`, is fitted by `A·g² + S·g` with both terms at least 0; then `σ² = A + S` and
/// `ρ = A / (A + S)`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BackgroundSplit {
    /// The variance where the flat did nothing, `g = 1`.
    pub variance: f32,
    /// The share of [`Self::variance`] the flat amplifies twice.
    pub read_share: f32,
}

impl BackgroundSplit {
    /// A frame no flat divided: its measured variance, which no gain scales.
    pub(crate) const fn unflattened(variance: f32) -> Self {
        Self {
            variance,
            read_share: 0.0,
        }
    }

    /// A frame no flat divided, of white noise `sigma`.
    pub const fn of_sigma(sigma: f32) -> Self {
        Self::unflattened(sigma * sigma)
    }

    /// The variance at flat gain `gain`.
    pub(crate) const fn at(self, gain: f32) -> f32 {
        self.variance * gain * (self.read_share * gain + 1.0 - self.read_share)
    }

    /// The standard deviation where the flat did nothing.
    pub fn sigma(self) -> f32 {
        self.variance.sqrt()
    }
}
