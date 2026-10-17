//! What the cosmic-ray detector thresholds against.

use crate::config_value::ConfigValue;
use crate::error::InvalidConfigField;

/// Laplacian-edge cosmic-ray detection parameters (van Dokkum 2001, L.A.Cosmic). Defaults match
/// ccdproc/astroscrappy; each field names L.A.Cosmic's parameter it is.
#[derive(Debug, Clone)]
pub struct CosmicRayConfig {
    /// `sigclip`, `σ_lim`: the Laplacian-to-noise significance a hit must reach (lower → more
    /// sensitive). Default 4.5.
    pub detection_sigma: f32,
    /// `objlim`, `f_lim`: the least contrast between a hit and the fine structure around it, which
    /// separates hits from stars a PSF broadens. Default 5.0.
    pub contrast_limit: f32,
    /// `sigfrac`: the share of `detection_sigma` a neighbour of a hit must reach to join it, which
    /// grows the mask onto the hit's fainter wings. Default 0.3.
    pub growth_fraction: f32,
    /// `niter`: the most detect→replace iterations; a multi-pixel hit needs several. Default 4.
    pub max_iterations: usize,
    /// How per-pixel noise is estimated for the significance image.
    pub noise: NoiseEstimation,
}

impl Default for CosmicRayConfig {
    fn default() -> Self {
        Self {
            detection_sigma: 4.5,
            contrast_limit: 5.0,
            growth_fraction: 0.3,
            max_iterations: 4,
            noise: NoiseEstimation::Measured,
        }
    }
}

impl CosmicRayConfig {
    /// Check every field against the range the detector can run with: positive thresholds, a
    /// growth fraction of the detection threshold, at least one pass, and a positive gain when one
    /// is given.
    pub(crate) fn validate(&self) -> Result<(), InvalidConfigField> {
        ConfigValue::new("cosmic-ray detection_sigma", self.detection_sigma)
            .finite_and("finite and positive", |v| v > 0.0)?;
        ConfigValue::new("cosmic-ray contrast_limit", self.contrast_limit)
            .finite_and("finite and positive", |v| v > 0.0)?;
        ConfigValue::new("cosmic-ray growth_fraction", self.growth_fraction)
            .finite_and("finite and in (0, 1]", |v| v > 0.0 && v <= 1.0)?;
        ConfigValue::count("cosmic-ray max_iterations", self.max_iterations)
            .must_be("at least 1", self.max_iterations >= 1)?;
        if let NoiseEstimation::Gain { electrons_per_adu } = self.noise {
            ConfigValue::new("cosmic-ray electrons_per_adu", electrons_per_adu)
                .finite_and("finite and positive", |v| v > 0.0)?;
        }
        Ok(())
    }
}

/// Where the per-pixel noise `N` of the significance image `S = L⁺/N` takes the camera's gain from
/// (the mono path adds a ½ for its ×2 subsample).
///
/// Either way `N² = σ² + max(m₅ − sky, 0)·k`, as astroscrappy's `√(m₅ + rn² + bkg)` in electrons:
/// `sky` and `σ` are the local background of the pixel's color, read from a tile mesh of the frame,
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
