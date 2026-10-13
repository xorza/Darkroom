//! What bounds a RANSAC search: how long it runs, and which hypotheses are physically credible.

use crate::config_value::ConfigValue;
use crate::error::InvalidConfigField;

/// Configuration for robust transform estimation.
#[derive(Debug, Clone)]
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
    /// Whether to refine promising hypotheses with LO-RANSAC. Default: true.
    pub local_optimization: bool,
    /// Maximum LO-RANSAC refinement iterations. Default: 10.
    pub lo_iterations: usize,
    /// Maximum absolute rotation in radians, or `None` for any. Default: `None` — a meridian flip
    /// turns the field by 180°, and an alt-az mount turns it through a session.
    pub max_rotation: Option<f64>,
    /// Accepted uniform-scale range, or `None` for any. Default: 0.8 to 1.2.
    pub scale_range: Option<ScaleRange>,
}

/// The uniform scales a hypothesis may have, both ends included.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScaleRange {
    pub min: f64,
    pub max: f64,
}

impl ScaleRange {
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
            local_optimization: true,
            lo_iterations: 10,
            max_rotation: None,
            scale_range: Some(ScaleRange { min: 0.8, max: 1.2 }),
        }
    }
}

impl RansacConfig {
    pub(crate) fn validate(&self) -> Result<(), InvalidConfigField> {
        ConfigValue::count("ransac max_iterations", self.max_iterations)
            .must_be("at least 1", self.max_iterations >= 1)?;
        ConfigValue::count("ransac lo_iterations", self.lo_iterations).must_be(
            "at least 1 when local_optimization is enabled",
            !self.local_optimization || self.lo_iterations >= 1,
        )?;
        ConfigValue::new("ransac confidence", self.confidence)
            .finite_and("finite and in [0, 1]", |value| (0.0..=1.0).contains(&value))?;
        ConfigValue::new("ransac min_inlier_ratio", self.min_inlier_ratio)
            .finite_and("finite and in (0, 1]", |value| value > 0.0 && value <= 1.0)?;
        if let Some(max_rotation) = self.max_rotation {
            ConfigValue::new("ransac max_rotation", max_rotation)
                .finite_and("finite and positive", |value| value > 0.0)?;
        }
        if let Some(ScaleRange { min, max }) = self.scale_range {
            ConfigValue::new("ransac scale_range minimum", min)
                .finite_and("finite and positive", |value| value > 0.0)?;
            ConfigValue::new("ransac scale_range maximum", max).must_be_against(
                "finite and above the minimum",
                min,
                max.is_finite() && max > min,
            )?;
        }
        Ok(())
    }
}
