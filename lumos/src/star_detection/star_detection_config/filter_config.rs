//! Final quality and duplicate-filtering settings.

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
    /// How far above the median FWHM a star may lie, in robust σ, `1.4826·MAD` of the brightest
    /// half's FWHMs, or `None` to keep every FWHM.
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
        InvalidConfigField::finite("min_snr", "finite and positive", self.min_snr, |value| {
            value > 0.0
        })?;
        InvalidConfigField::finite(
            "max_eccentricity",
            "finite and in [0, 1]",
            self.max_eccentricity,
            |value| (0.0..=1.0).contains(&value),
        )?;
        InvalidConfigField::finite(
            "max_sharpness",
            "finite and in (0, 1]",
            self.max_sharpness,
            |value| value > 0.0 && value <= 1.0,
        )?;
        InvalidConfigField::finite(
            "max_roundness",
            "finite and in (0, 2]",
            self.max_roundness,
            |value| value > 0.0 && value <= 2.0,
        )?;
        if let Some(deviation) = self.max_fwhm_deviation {
            InvalidConfigField::finite(
                "max_fwhm_deviation",
                "finite and positive",
                deviation,
                |value| value > 0.0,
            )?;
        }
        InvalidConfigField::finite(
            "duplicate_min_separation",
            "finite and non-negative",
            self.duplicate_min_separation,
            |value| value >= 0.0,
        )?;
        Ok(())
    }
}
