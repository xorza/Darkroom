//! [`BackgroundSplit`]: a frame's background noise split by how a flat amplified it.

use crate::noise::binned_samples::GainBin;

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
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct BackgroundSplit {
    /// The variance where the flat did nothing, `g = 1`.
    pub(crate) variance: f32,
    /// The share of [`Self::variance`] the flat amplifies twice.
    pub(crate) read_share: f32,
}

impl BackgroundSplit {
    /// A frame no flat divided: its measured variance, which no gain scales.
    pub(crate) const fn unflattened(variance: f32) -> Self {
        Self {
            variance,
            read_share: 0.0,
        }
    }

    /// The variance at flat gain `gain`.
    pub(crate) const fn at(self, gain: f32) -> f32 {
        self.variance * gain * (self.read_share * gain + 1.0 - self.read_share)
    }

    /// The weighted least-squares fit of `A·g² + S·g` to `bins`, both terms at least 0, each bin
    /// weighed by its samples over its variance squared: the inverse of its estimate's sampling
    /// variance, `2v²/n` for Gaussian noise.
    ///
    /// When the unconstrained solution leaves a term negative, the better of the two one-term fits
    /// stands. Where the gain hardly varies, the two terms cannot be told apart — and there the
    /// split changes no variance the frame holds — so a tie goes to the sky-limited `S·g`, the
    /// exposure most deep-sky frames are taken at. No bin at all reads 0.
    pub(crate) fn fit(bins: &[GainBin]) -> Self {
        if !bins.iter().any(|bin| bin.samples > 0) {
            return Self::unflattened(0.0);
        }
        let g4 = weighted_sum(bins, |g, _| g.powi(4));
        let g3 = weighted_sum(bins, |g, _| g.powi(3));
        let g2 = weighted_sum(bins, |g, _| g * g);
        let vg2 = weighted_sum(bins, |g, v| v * g * g);
        let vg = weighted_sum(bins, |g, v| v * g);
        let residual =
            |read: f64, sky: f64| weighted_sum(bins, |g, v| (v - read * g * g - sky * g).powi(2));
        let sky_only = (0.0, vg / g2);
        let read_only = (vg2 / g4, 0.0);
        let one_term = if residual(read_only.0, read_only.1) < residual(sky_only.0, sky_only.1) {
            read_only
        } else {
            sky_only
        };
        let determinant = g4 * g2 - g3 * g3;
        // Each sum rounds by parts in 10¹⁶; a determinant within 10⁻¹² of the products it is the
        // difference of is a gain spread under 10⁻⁶, where the two-term solve reads rounding.
        let (read, sky) = if determinant > 1e-12 * g4 * g2 {
            let read = (vg2 * g2 - vg * g3) / determinant;
            let sky = (g4 * vg - g3 * vg2) / determinant;
            if read >= 0.0 && sky >= 0.0 {
                (read, sky)
            } else {
                one_term
            }
        } else {
            one_term
        };
        let variance = read + sky;
        Self {
            variance: variance as f32,
            read_share: if variance > 0.0 {
                (read / variance) as f32
            } else {
                0.0
            },
        }
    }
}

/// `Σ w·term(g, v)` over the bins that hold samples, at each bin's gain `g` and variance `v`: `w`
/// its samples over `v²`, or its samples alone where `v` is 0.
fn weighted_sum(bins: &[GainBin], term: impl Fn(f64, f64) -> f64) -> f64 {
    bins.iter()
        .filter(|bin| bin.samples > 0)
        .map(|bin| {
            let weight = if bin.variance > 0.0 {
                bin.samples as f64 / (bin.variance * bin.variance)
            } else {
                bin.samples as f64
            };
            weight * term(bin.gain, bin.variance)
        })
        .sum::<f64>()
}

#[cfg(test)]
mod tests;
