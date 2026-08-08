//! Star detection result types.

use glam::DVec2;
use serde::{Deserialize, Serialize};

use crate::stacking::star_detection::roundness::Roundness;

/// A detected star with sub-pixel position and quality metrics.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Star {
    /// Position (sub-pixel accurate).
    pub pos: DVec2,
    /// Total flux (sum of background-subtracted pixel values).
    pub flux: f32,
    /// Full Width at Half Maximum in pixels.
    pub fwhm: f32,
    /// Eccentricity (0 = circular, 1 = elongated). Used to reject non-stellar objects.
    pub eccentricity: f32,
    /// Signal-to-noise ratio.
    pub snr: f32,
    /// Peak pixel value (for saturation detection).
    pub peak: f32,
    /// Sharpness metric (peak / flux_in_core). Cosmic rays have high sharpness (>0.8),
    /// real stars have lower sharpness (typically 0.2-0.6 depending on seeing).
    pub sharpness: f32,
    /// The DAOFIND roundness metrics.
    pub roundness: Roundness,
}

/// Default peak threshold (normalized) above which a star is treated as saturated.
/// Saturated peaks give unreliable centroids, so the detection quality filters drop them.
pub(crate) const SATURATION_PEAK: f32 = 0.95;

impl Star {
    /// Check if star is likely saturated.
    ///
    /// Stars with peak values near the maximum have unreliable centroids.
    /// Typical threshold: 0.95 for normalized data.
    pub fn is_saturated(&self, threshold: f32) -> bool {
        self.peak > threshold
    }

    /// Check if star is likely a cosmic ray (very sharp, single-pixel spike).
    ///
    /// Cosmic rays typically have sharpness > 0.7, while real stars are 0.2-0.5.
    pub fn is_cosmic_ray(&self, max_sharpness: f32) -> bool {
        self.sharpness > max_sharpness
    }

    /// Check if star passes roundness filters.
    ///
    /// Both roundness metrics should be close to zero for circular sources.
    pub fn is_round(&self, max_roundness: f32) -> bool {
        self.roundness.ground.abs() <= max_roundness && self.roundness.sround.abs() <= max_roundness
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use glam::DVec2;

    use crate::stacking::star_detection::roundness::Roundness;
    use crate::stacking::star_detection::star::Star;

    impl Star {
        /// A clean star at `pos` — the base every test fixture builds on, overriding only the
        /// fields its assertion is about.
        ///
        /// The defaults clear every [`FilterConfig`](crate::stacking::star_detection::config::filter_config::FilterConfig)
        /// default with room to spare, so any rejection a test observes is the one it asked for.
        pub(crate) fn at(pos: DVec2) -> Self {
            Self {
                pos,
                flux: 100.0,
                fwhm: 3.0,
                eccentricity: 0.1,
                snr: 50.0,
                peak: 0.5,
                sharpness: 0.3,
                roundness: Roundness {
                    ground: 0.0,
                    sround: 0.0,
                },
            }
        }

        pub(crate) fn with_pos(mut self, pos: DVec2) -> Self {
            self.pos = pos;
            self
        }

        pub(crate) fn with_flux(mut self, flux: f32) -> Self {
            self.flux = flux;
            self
        }

        pub(crate) fn with_fwhm(mut self, fwhm: f32) -> Self {
            self.fwhm = fwhm;
            self
        }

        pub(crate) fn with_eccentricity(mut self, eccentricity: f32) -> Self {
            self.eccentricity = eccentricity;
            self
        }

        pub(crate) fn with_snr(mut self, snr: f32) -> Self {
            self.snr = snr;
            self
        }

        pub(crate) fn with_peak(mut self, peak: f32) -> Self {
            self.peak = peak;
            self
        }

        pub(crate) fn with_sharpness(mut self, sharpness: f32) -> Self {
            self.sharpness = sharpness;
            self
        }

        pub(crate) fn with_roundness(mut self, roundness: Roundness) -> Self {
            self.roundness = roundness;
            self
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::stacking::star_detection::star::*;

    /// Position is irrelevant to the three predicates below; each test sets only its own field.
    fn star() -> Star {
        Star::at(DVec2::ZERO)
    }

    #[test]
    fn saturation_compares_peak_against_the_given_threshold() {
        assert!(star().with_peak(0.96).is_saturated(0.95));
        // Strictly greater, so a peak sitting exactly on the threshold is not saturated.
        assert!(!star().with_peak(0.95).is_saturated(0.95));
        assert!(!star().with_peak(0.5).is_saturated(0.95));
        // The threshold decides, not the peak: one peak, both verdicts.
        assert!(star().with_peak(0.85).is_saturated(0.80));
        assert!(!star().with_peak(0.85).is_saturated(0.90));
    }

    #[test]
    fn cosmic_ray_compares_sharpness_against_the_given_threshold() {
        assert!(star().with_sharpness(0.8).is_cosmic_ray(0.7));
        assert!(!star().with_sharpness(0.7).is_cosmic_ray(0.7));
        assert!(!star().with_sharpness(0.3).is_cosmic_ray(0.7));
    }

    #[test]
    fn roundness_requires_both_metrics_within_the_threshold() {
        let round = |ground, sround| star().with_roundness(Roundness { ground, sround });

        assert!(round(0.0, 0.0).is_round(0.3));
        // Compared by magnitude, so both metrics sitting on the bound with opposite signs pass.
        assert!(round(0.3, -0.3).is_round(0.3));
        // Either one over the bound fails the whole check.
        assert!(!round(0.5, 0.0).is_round(0.3));
        assert!(!round(0.0, -0.5).is_round(0.3));
    }
}
