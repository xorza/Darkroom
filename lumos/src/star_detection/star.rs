//! Star detection result types.

use glam::DVec2;
use serde::{Deserialize, Serialize};

use crate::star_detection::roundness::Roundness;

/// A detected star with sub-pixel position and quality metrics.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Star {
    /// Position (sub-pixel accurate).
    pub pos: DVec2,
    /// The position's standard error, `√((σ_x² + σ_y²)/2)` in pixels: from the profile fit's
    /// covariance `(JᵀWJ)⁻¹·χ²/(n − p)`, or from the windowed centroid's propagated pixel noise when
    /// no fit ran or it failed.
    pub position_sigma: f64,
    /// Total flux (sum of background-subtracted pixel values).
    pub flux: f32,
    /// The PSF's full width at half maximum, in pixels, before the pixel integrated it: the width
    /// the profile fits model, and the moments read once the pixel's own variance is removed. 0
    /// for a source the moments place inside one pixel.
    pub fwhm: f32,
    /// Eccentricity (0 = circular, 1 = elongated), of the same PSF. Used to reject non-stellar
    /// objects; 0 for a source the moments place inside one pixel.
    pub eccentricity: f32,
    /// Signal-to-noise ratio.
    pub snr: f32,
    /// Peak pixel value above the sky.
    pub peak: f32,
    /// Whether the peak pixel reached the saturation level: its centroid and flux are unreliable.
    pub saturated: bool,
    /// Sharpness: the star's own peak, at its centre pixel, over the 3 × 3 core's flux about it.
    /// A cosmic ray, a single pixel, reads near 1; a star spreads its light, 0.14 for a centred
    /// Gaussian of FWHM 4.
    pub sharpness: f32,
    /// The DAOFIND roundness metrics.
    pub roundness: Roundness,
}

impl Star {
    /// Check if star is likely a cosmic ray: sharper than `max_sharpness`, the filter's 0.7 by
    /// default.
    pub const fn is_cosmic_ray(&self, max_sharpness: f32) -> bool {
        self.sharpness > max_sharpness
    }

    /// Check if star passes roundness filters.
    ///
    /// Both roundness metrics should be close to zero for circular sources.
    pub const fn is_round(&self, max_roundness: f32) -> bool {
        self.roundness.ground.abs() <= max_roundness && self.roundness.sround.abs() <= max_roundness
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use glam::DVec2;

    use crate::star_detection::roundness::Roundness;
    use crate::star_detection::star::Star;

    impl Star {
        /// A clean star at `pos` — the base every test fixture builds on, overriding only the
        /// fields its assertion is about.
        ///
        /// The defaults clear every
        /// [`FilterConfig`](crate::star_detection::config::filter_config::FilterConfig)
        /// default with room to spare, so any rejection a test observes is the one it asked for.
        pub(crate) fn at(pos: DVec2) -> Self {
            Self {
                pos,
                position_sigma: 0.01,
                flux: 100.0,
                fwhm: 3.0,
                eccentricity: 0.1,
                snr: 50.0,
                peak: 0.5,
                saturated: false,
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

        pub(crate) fn with_saturated(mut self, saturated: bool) -> Self {
            self.saturated = saturated;
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
    use crate::star_detection::star::*;

    /// Position is irrelevant to the two predicates below; each test sets only its own field.
    fn star() -> Star {
        Star::at(DVec2::ZERO)
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
