//! What bounds a RANSAC search: how long it runs, and which hypotheses are physically credible.

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
    /// Random seed for reproducible sampling. Default: random.
    pub seed: Option<u64>,
    /// Whether to refine promising hypotheses with LO-RANSAC. Default: true.
    pub local_optimization: bool,
    /// Maximum LO-RANSAC refinement iterations. Default: 10.
    pub lo_iterations: usize,
    /// Maximum absolute rotation in radians. Default: 10 degrees.
    pub max_rotation: Option<f64>,
    /// Accepted uniform-scale range. Default: 0.8 to 1.2.
    pub scale_range: Option<(f64, f64)>,
}

impl Default for RansacConfig {
    fn default() -> Self {
        Self {
            max_iterations: 2000,
            confidence: 0.995,
            min_inlier_ratio: 0.3,
            seed: None,
            local_optimization: true,
            lo_iterations: 10,
            max_rotation: Some(10.0_f64.to_radians()),
            scale_range: Some((0.8, 1.2)),
        }
    }
}

impl RansacConfig {
    pub(crate) fn validate(&self) -> Result<(), InvalidConfigField> {
        InvalidConfigField::check(
            self.max_iterations >= 1,
            "ransac max_iterations",
            "at least 1",
            self.max_iterations as f64,
        )?;
        InvalidConfigField::check(
            !self.local_optimization || self.lo_iterations >= 1,
            "ransac lo_iterations",
            "at least 1 when local_optimization is enabled",
            self.lo_iterations as f64,
        )?;
        InvalidConfigField::finite(
            "ransac confidence",
            "finite and in [0, 1]",
            self.confidence,
            |value| (0.0..=1.0).contains(&value),
        )?;
        InvalidConfigField::finite(
            "ransac min_inlier_ratio",
            "finite and in (0, 1]",
            self.min_inlier_ratio,
            |value| value > 0.0 && value <= 1.0,
        )?;
        if let Some(max_rotation) = self.max_rotation {
            InvalidConfigField::finite(
                "ransac max_rotation",
                "finite and positive",
                max_rotation,
                |value| value > 0.0,
            )?;
        }
        if let Some((min_scale, max_scale)) = self.scale_range {
            InvalidConfigField::finite(
                "ransac scale_range minimum",
                "finite and positive",
                min_scale,
                |value| value > 0.0,
            )?;
            InvalidConfigField::check_against(
                max_scale.is_finite() && max_scale > min_scale,
                "ransac scale_range maximum",
                "finite and above the minimum",
                max_scale,
                min_scale,
            )?;
        }
        Ok(())
    }
}
