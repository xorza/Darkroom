//! What bounds a RANSAC search: how long it runs, and which hypotheses are physically credible.

use std::num::NonZeroUsize;

use crate::config_field::ConfigField;
use crate::error::InvalidConfigField;

/// Configuration for robust transform estimation.
#[derive(Debug, Clone, Copy)]
pub struct RansacConfig {
    /// Maximum hypotheses to evaluate. Default: 2000.
    pub max_iterations: usize,
    /// Target confidence for adaptive early termination. Default: 0.995.
    pub confidence: f64,
    /// Minimum inlier ratio before adaptive early termination. Default: 0.3.
    pub min_inlier_ratio: f64,
    /// The sampler's seed: the same seed on the same input gives the same transform, bit for bit.
    /// Default: 0.
    pub seed: u64,
    /// The LO-RANSAC refinement iterations a new best hypothesis gets at most, `None` for no
    /// refinement. Default: 10.
    pub local_optimization: Option<NonZeroUsize>,
    /// Maximum absolute rotation in radians, or `None` for any. Default: `None` — a meridian flip
    /// turns the field by 180°, and an alt-az mount turns it through a session.
    pub max_rotation_rad: Option<f64>,
    /// Accepted uniform-scale range, or `None` for any. Default: 0.8 to 1.2.
    pub scale_range: Option<ScaleRange>,
}

/// The uniform scales a hypothesis may have, both ends included: a finite, positive minimum
/// below a finite maximum, by [`Self::checked`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScaleRange {
    min: f64,
    max: f64,
}

impl ScaleRange {
    /// The scales from `min` to `max`.
    ///
    /// # Panics
    /// When [`Self::checked`] refuses them.
    pub fn new(min: f64, max: f64) -> Self {
        Self::checked(min, max).unwrap_or_else(|invalid| panic!("{invalid}"))
    }

    /// [`Self::new`], or the end that is invalid.
    ///
    /// # Errors
    ///
    /// A minimum that is not finite and positive, or a maximum that is not finite and above it.
    pub fn checked(min: f64, max: f64) -> Result<Self, InvalidConfigField> {
        ConfigField::new("ransac scale_range minimum", min)
            .finite_and("finite and positive", |value| value > 0.0)?;
        ConfigField::new("ransac scale_range maximum", max).must_be_against(
            "finite and above the minimum",
            min,
            max.is_finite() && max > min,
        )?;
        Ok(Self { min, max })
    }

    pub const fn min(self) -> f64 {
        self.min
    }

    pub const fn max(self) -> f64 {
        self.max
    }

    /// Whether `scale` lies outside the range.
    pub(crate) const fn excludes(self, scale: f64) -> bool {
        scale < self.min || scale > self.max
    }
}

impl Default for RansacConfig {
    fn default() -> Self {
        Self {
            max_iterations: 2000,
            confidence: 0.995,
            min_inlier_ratio: 0.3,
            seed: 0,
            local_optimization: NonZeroUsize::new(10),
            max_rotation_rad: None,
            scale_range: Some(ScaleRange::new(0.8, 1.2)),
        }
    }
}

impl RansacConfig {
    pub(crate) fn validate(&self) -> Result<(), InvalidConfigField> {
        ConfigField::count("ransac max_iterations", self.max_iterations)
            .must_be("at least 1", self.max_iterations >= 1)?;
        // Open at both ends: `ln(1 − p)` is 0 at 0, which asks for no iteration at all, and −∞ at
        // 1, which asks for infinitely many.
        ConfigField::new("ransac confidence", self.confidence)
            .finite_and("finite and in (0, 1)", |value| value > 0.0 && value < 1.0)?;
        ConfigField::new("ransac min_inlier_ratio", self.min_inlier_ratio)
            .finite_and("finite and in (0, 1]", |value| value > 0.0 && value <= 1.0)?;
        if let Some(max_rotation_rad) = self.max_rotation_rad {
            ConfigField::new("ransac max_rotation_rad", max_rotation_rad)
                .finite_and("finite and positive", |value| value > 0.0)?;
        }
        Ok(())
    }
}
