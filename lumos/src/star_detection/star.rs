//! Star detection result types.

use glam::DVec2;

/// A detected star with sub-pixel position and quality metrics.
#[derive(Debug, Clone, Copy)]
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
    /// Roundness based on marginal Gaussian fits (DAOFIND GROUND).
    /// (Hx - Hy) / (Hx + Hy) where Hx, Hy are heights of marginal x and y fits.
    /// Circular sources → 0, x-extended → negative, y-extended → positive.
    pub roundness1: f32,
    /// Roundness based on symmetry (DAOFIND SROUND).
    /// Measures bilateral vs four-fold symmetry. Circular → 0, asymmetric → non-zero.
    pub roundness2: f32,
}

impl Star {
    /// Check if star is likely saturated.
    ///
    /// Stars with peak values near the maximum (>0.95 for normalized data)
    /// have unreliable centroids.
    pub fn is_saturated(&self) -> bool {
        self.peak > 0.95
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
        self.roundness1.abs() <= max_roundness && self.roundness2.abs() <= max_roundness
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_star() -> Star {
        Star {
            pos: DVec2::new(10.0, 10.0),
            flux: 100.0,
            fwhm: 3.0,
            eccentricity: 0.1,
            snr: 50.0,
            peak: 0.5,
            sharpness: 0.3,
            roundness1: 0.0,
            roundness2: 0.0,
        }
    }

    #[test]
    fn test_is_saturated() {
        assert!(
            Star {
                peak: 0.96,
                ..make_test_star()
            }
            .is_saturated()
        );
        assert!(
            !Star {
                peak: 0.95,
                ..make_test_star()
            }
            .is_saturated()
        );
        assert!(
            !Star {
                peak: 0.5,
                ..make_test_star()
            }
            .is_saturated()
        );
    }

    #[test]
    fn test_is_cosmic_ray() {
        assert!(
            Star {
                sharpness: 0.8,
                ..make_test_star()
            }
            .is_cosmic_ray(0.7)
        );
        assert!(
            !Star {
                sharpness: 0.7,
                ..make_test_star()
            }
            .is_cosmic_ray(0.7)
        );
        assert!(
            !Star {
                sharpness: 0.3,
                ..make_test_star()
            }
            .is_cosmic_ray(0.7)
        );
    }

    #[test]
    fn test_is_round() {
        // Both roundness values within threshold
        assert!(
            Star {
                roundness1: 0.0,
                roundness2: 0.0,
                ..make_test_star()
            }
            .is_round(0.3)
        );
        assert!(
            Star {
                roundness1: 0.3,
                roundness2: -0.3,
                ..make_test_star()
            }
            .is_round(0.3)
        );

        // roundness1 exceeds threshold
        assert!(
            !Star {
                roundness1: 0.5,
                roundness2: 0.0,
                ..make_test_star()
            }
            .is_round(0.3)
        );

        // roundness2 exceeds threshold
        assert!(
            !Star {
                roundness1: 0.0,
                roundness2: -0.5,
                ..make_test_star()
            }
            .is_round(0.3)
        );
    }
}
