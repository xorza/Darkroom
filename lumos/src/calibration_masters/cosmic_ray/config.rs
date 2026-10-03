//! What the cosmic-ray detector thresholds against.

use crate::error::InvalidConfigField;

/// Laplacian-edge cosmic-ray detection parameters. Defaults match ccdproc/astroscrappy.
#[derive(Debug, Clone)]
pub struct CosmicRayConfig {
    /// `σ_lim`: Laplacian-to-noise significance threshold (lower → more sensitive). Default 4.5.
    pub sigclip: f32,
    /// `f_lim`: minimum CR-to-fine-structure contrast separating CRs from PSF-broadened stars.
    /// Default 5.0.
    pub objlim: f32,
    /// Fraction of `sigclip` used when growing the mask onto a flagged CR's fainter wings. Default
    /// 0.3.
    pub sigfrac: f32,
    /// Maximum detect→replace iterations (multi-pixel CRs need several). Default 4.
    pub niter: usize,
    /// How per-pixel noise is estimated for the significance image.
    pub noise: NoiseEstimation,
}

impl Default for CosmicRayConfig {
    fn default() -> Self {
        Self {
            sigclip: 4.5,
            objlim: 5.0,
            sigfrac: 0.3,
            niter: 4,
            noise: NoiseEstimation::Measured,
        }
    }
}

impl CosmicRayConfig {
    /// Check every field against the range the detector can run with: positive thresholds, a
    /// growth fraction of the detection threshold, at least one pass, and a positive gain when one
    /// is given.
    pub(crate) fn validate(&self) -> Result<(), InvalidConfigField> {
        InvalidConfigField::finite(
            "cosmic-ray sigclip",
            "finite and positive",
            self.sigclip,
            |v| v > 0.0,
        )?;
        InvalidConfigField::finite(
            "cosmic-ray objlim",
            "finite and positive",
            self.objlim,
            |v| v > 0.0,
        )?;
        InvalidConfigField::finite(
            "cosmic-ray sigfrac",
            "finite and in (0, 1]",
            self.sigfrac,
            |v| v > 0.0 && v <= 1.0,
        )?;
        InvalidConfigField::check(
            self.niter >= 1,
            "cosmic-ray niter",
            "at least 1",
            self.niter as f64,
        )?;
        if let NoiseEstimation::Gain { electrons_per_adu } = self.noise {
            InvalidConfigField::finite(
                "cosmic-ray electrons_per_adu",
                "finite and positive",
                electrons_per_adu,
                |v| v > 0.0,
            )?;
        }
        Ok(())
    }
}

/// Where the per-pixel noise `N` of the significance image `S = L⁺/N` takes the camera's gain from
/// (the mono path adds a ½ for its ×2 subsample).
///
/// Either way `N² = σ² + max(m₅ − sky, 0)·k`, as astroscrappy's `√(m₅ + rn² + bkg)` in electrons:
/// `sky` and `σ` are the local background of the pixel's colour, read from a tile mesh of the frame,
/// so σ already holds the read noise, the subtracted dark's and sky's photon noise and the
/// quantization; `m₅` is the median-filtered signal, and `k` is the variance one unit of signal
/// above the sky adds, `1/electrons_per_unit`.
#[derive(Debug, Clone)]
pub enum NoiseEstimation {
    /// The gain the frame states (its `EGAIN` over a declared scale). Without one, `k` is
    /// `σ²/max(sky, σ)`, which takes the background as sky-photon-dominated: on read-noise-dominated
    /// frames it over-estimates the noise in bright regions and under-flags there slightly.
    Measured,
    /// A gain the caller states, in e⁻/ADU. The ADU one sample unit is worth comes from the frame:
    /// its declared scale, else the ADC step its decoder recorded as the quantization σ, and a frame
    /// with neither cannot use this.
    Gain { electrons_per_adu: f32 },
}
