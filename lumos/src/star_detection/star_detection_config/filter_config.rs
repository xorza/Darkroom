//! Final quality and duplicate-filtering settings.

use crate::config_field::ConfigField;
use crate::error::InvalidConfigField;

/// Configuration for final star-quality filtering and duplicate removal.
#[derive(Debug, Clone)]
pub struct FilterConfig {
    /// Minimum accepted signal-to-noise ratio.
    pub min_snr: f32,
    /// Maximum accepted eccentricity.
    pub max_eccentricity: f32,
    /// Maximum accepted sharpness.
    pub max_sharpness: f32,
    /// Maximum accepted absolute roundness, on both DAOFIND metrics. Each lies in `[−2, 2]`, so 2
    /// keeps every shape.
    pub max_roundness: f32,
    /// How far above the median FWHM a star may lie, in the robust σ of the brightest half's
    /// FWHMs, from their MAD, or `None` to keep every FWHM.
    pub max_fwhm_deviation: Option<f32>,
    /// Minimum retained separation between duplicate stars in pixels.
    pub duplicate_min_separation: f32,
}

impl Default for FilterConfig {
    fn default() -> Self {
        Self {
            min_snr: 10.0,
            max_eccentricity: 0.6,
            max_sharpness: 0.7,
            max_roundness: 0.5,
            max_fwhm_deviation: Some(3.0),
            duplicate_min_separation: 8.0,
        }
    }
}

impl FilterConfig {
    pub(super) fn validate(&self) -> Result<(), InvalidConfigField> {
        ConfigField::new("min_snr", self.min_snr)
            .finite_and("finite and positive", |value| value > 0.0)?;
        ConfigField::new("max_eccentricity", self.max_eccentricity)
            .finite_and("finite and in [0, 1]", |value| (0.0..=1.0).contains(&value))?;
        ConfigField::new("max_sharpness", self.max_sharpness)
            .finite_and("finite and in (0, 1]", |value| value > 0.0 && value <= 1.0)?;
        ConfigField::new("max_roundness", self.max_roundness)
            .finite_and("finite and in (0, 2]", |value| value > 0.0 && value <= 2.0)?;
        if let Some(deviation) = self.max_fwhm_deviation {
            ConfigField::new("max_fwhm_deviation", deviation)
                .finite_and("finite and positive", |value| value > 0.0)?;
        }
        ConfigField::new("duplicate_min_separation", self.duplicate_min_separation)
            .finite_and("finite and non-negative", |value| value >= 0.0)?;
        Ok(())
    }
}
