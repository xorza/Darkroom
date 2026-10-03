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
            noise: NoiseEstimation::Empirical,
        }
    }
}

impl CosmicRayConfig {
    /// Check every field against the range the detector can run with: positive thresholds, a
    /// growth fraction of the detection threshold, at least one pass, and a camera model with a
    /// positive gain and a read noise that is not negative.
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
        if let NoiseEstimation::Parametric { gain, read_noise } = self.noise {
            InvalidConfigField::finite("cosmic-ray gain", "finite and positive", gain, |v| {
                v > 0.0
            })?;
            InvalidConfigField::finite(
                "cosmic-ray read_noise",
                "finite and not negative",
                read_noise,
                |v| v >= 0.0,
            )?;
        }
        Ok(())
    }
}

/// Per-pixel noise `N` for the significance image `S = L⁺/N` (the mono path adds a ½ for its ×2
/// subsample). Shared by all CFA paths.
#[derive(Debug, Clone)]
pub enum NoiseEstimation {
    /// Self-calibrating: a robust background σ (MAD) as the read-noise floor, scaled by the
    /// median-filtered signal for the Poisson term. Needs no camera parameters (default).
    ///
    /// This is a pragmatic approximation, **not** the canonical L.A.Cosmic noise model — ccdproc/
    /// astroscrappy always work in electrons (use [`NoiseEstimation::Parametric`] for that). It
    /// assumes a **sky-Poisson-dominated background** (the Poisson slope is anchored at the
    /// background, `σ_bg²/bg`), so on read-noise-dominated frames it over-estimates noise in bright
    /// regions and therefore slightly *under*-flags there. Chosen as the default because `gain`/
    /// `read_noise` are often unknown or unreliable for normalized data.
    Empirical,
    /// Exact Poisson + read noise `N_e = √(gain·I_ADU + read_noise²)`. The ADU one sample unit is
    /// worth comes from the frame: its decoder records the ADC step as the frame's quantization
    /// σ, and a frame without one cannot use this model.
    Parametric {
        /// e⁻/ADU.
        gain: f32,
        /// Read noise, e⁻.
        read_noise: f32,
    },
}
