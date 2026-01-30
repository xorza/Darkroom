//! Star detection result types.

/// A detected star with sub-pixel position and quality metrics.
#[derive(Debug, Clone, Copy)]
pub struct Star {
    /// X coordinate (sub-pixel accurate).
    pub x: f32,
    /// Y coordinate (sub-pixel accurate).
    pub y: f32,
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
    /// L.A.Cosmic Laplacian SNR metric for cosmic ray detection.
    /// Cosmic rays have very sharp edges (high Laplacian), stars have smooth edges.
    /// Values > 50 typically indicate cosmic rays. Based on van Dokkum 2001.
    pub laplacian_snr: f32,
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

    /// Check if star is likely a cosmic ray using L.A.Cosmic Laplacian metric.
    ///
    /// Based on van Dokkum 2001: cosmic rays have very sharp edges that produce
    /// high Laplacian values. Stars, being smoothed by the PSF, have lower values.
    /// Threshold of ~50 is typical for rejecting cosmic rays.
    pub fn is_cosmic_ray_laplacian(&self, max_laplacian_snr: f32) -> bool {
        self.laplacian_snr > max_laplacian_snr
    }

    /// Check if star passes roundness filters.
    ///
    /// Both roundness metrics should be close to zero for circular sources.
    pub fn is_round(&self, max_roundness: f32) -> bool {
        self.roundness1.abs() <= max_roundness && self.roundness2.abs() <= max_roundness
    }

    /// Check if star passes quality filters for registration.
    ///
    /// Filters out saturated, elongated, low-SNR stars, cosmic rays, and non-round objects.
    /// Unlike simple `is_*` predicates, this method combines multiple quality criteria.
    pub fn passes_quality_filters(
        &self,
        min_snr: f32,
        max_eccentricity: f32,
        max_sharpness: f32,
        max_roundness: f32,
    ) -> bool {
        !self.is_saturated()
            && self.snr >= min_snr
            && self.eccentricity <= max_eccentricity
            && !self.is_cosmic_ray(max_sharpness)
            && self.is_round(max_roundness)
    }
}
