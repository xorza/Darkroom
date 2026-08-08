//! Matched-filter FWHM selection and estimation settings.

use crate::error::InvalidConfigField;

/// Configuration for selecting or estimating the matched-filter FWHM.
#[derive(Debug, Clone)]
pub struct FwhmConfig {
    /// Fixed matched-filter FWHM, or the fallback for auto-estimation; zero disables it.
    pub expected: f32,
    /// Whether to estimate FWHM from a first-pass star catalog.
    pub auto_estimate: bool,
    /// Minimum first-pass stars required to accept an estimate.
    pub min_stars: usize,
    /// Multiplier applied to the detection threshold during the first pass.
    pub estimation_sigma_factor: f32,
}

impl Default for FwhmConfig {
    fn default() -> Self {
        Self {
            expected: 4.0,
            auto_estimate: false,
            min_stars: 10,
            estimation_sigma_factor: 2.0,
        }
    }
}

impl FwhmConfig {
    pub(super) fn validate(&self) -> Result<(), InvalidConfigField> {
        InvalidConfigField::finite(
            "expected_fwhm",
            "finite and non-negative",
            self.expected,
            |value| value >= 0.0,
        )?;
        InvalidConfigField::check(
            self.min_stars >= 5,
            "min_stars_for_fwhm",
            "at least 5",
            self.min_stars as f64,
        )?;
        InvalidConfigField::finite(
            "fwhm_estimation_sigma_factor",
            "finite and at least 1",
            self.estimation_sigma_factor,
            |value| value >= 1.0,
        )?;
        Ok(())
    }
}
