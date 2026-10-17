//! [`SigmaThreshold`]: how far above its dark's background a pixel stands before it is hot.

use crate::config_value::ConfigValue;
use crate::error::InvalidConfigField;

/// How many σ above the per-color residual median of a master dark a pixel stands before it is
/// flagged hot: finite, and at least 1, as nothing below 1σ is a defect — a 1σ cut alone flags
/// about 16 % of a Gaussian sensor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SigmaThreshold(f32);

impl SigmaThreshold {
    /// 5σ. PixInsight uses 3; 5 is more conservative, with fewer false positives.
    pub const DEFAULT: Self = Self(5.0);

    /// `sigma` as a threshold.
    ///
    /// # Errors
    ///
    /// [`InvalidConfigField`] when `sigma` is not finite or is below 1.
    pub fn new(sigma: f32) -> Result<Self, InvalidConfigField> {
        ConfigValue::new("sigma_threshold", sigma)
            .finite_and("finite and at least 1", |sigma| sigma >= 1.0)?;
        Ok(Self(sigma))
    }

    /// The threshold, in σ.
    pub const fn get(self) -> f32 {
        self.0
    }
}

impl Default for SigmaThreshold {
    fn default() -> Self {
        Self::DEFAULT
    }
}

#[cfg(test)]
mod tests {
    use crate::calibration::defect_map::sigma_threshold::SigmaThreshold;

    /// 1 and anything above it is a threshold; below 1, 0, a negative value and NaN are refused
    /// rather than clamped to 1σ, which would flag about 16 % of the sensor.
    #[test]
    fn a_threshold_is_finite_and_at_least_one_sigma() {
        assert_eq!(SigmaThreshold::new(1.0).unwrap().get(), 1.0);
        assert_eq!(SigmaThreshold::new(8.0).unwrap().get(), 8.0);
        for sigma in [0.999, 0.0, -5.0, f32::NAN, f32::INFINITY] {
            let refused = SigmaThreshold::new(sigma).unwrap_err();
            assert_eq!(refused.field, "sigma_threshold", "{sigma}");
        }
        assert_eq!(SigmaThreshold::default().get(), 5.0);
    }
}
