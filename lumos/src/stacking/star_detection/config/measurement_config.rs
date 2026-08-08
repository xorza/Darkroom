//! Centroid and metric measurement settings.
//!
//! The three types below are this config's own fields: how to centroid, how to take a local
//! background, and the optional sensor noise model that turns ADU into electrons.

use crate::error::InvalidConfigField;

/// Method for computing sub-pixel centroids.
///
/// Different methods offer tradeoffs between accuracy and speed.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum CentroidMethod {
    /// Iterative weighted centroid using Gaussian weights.
    /// Fast (~0.05 pixel accuracy). This is the default.
    #[default]
    WeightedMoments,

    /// 2D Gaussian profile fitting via Levenberg-Marquardt optimization.
    /// High precision (~0.01 pixel accuracy) but ~8x slower than WeightedMoments.
    /// Best for well-sampled, symmetric PSFs.
    GaussianFit,

    /// 2D Moffat profile fitting with configurable beta parameter.
    /// High precision (~0.01 pixel accuracy), similar speed to GaussianFit.
    /// Better model for atmospheric seeing (extended wings).
    /// Beta parameter controls wing slope: 2.5 typical for ground-based, 4.5 for space-based.
    MoffatFit {
        /// Power law slope controlling wing falloff. Typical range: 2.0-5.0.
        /// Lower values = more extended wings.
        beta: f32,
    },
}

impl CentroidMethod {
    /// Validate the centroid method configuration.
    pub fn validate(&self) -> Result<(), InvalidConfigField> {
        if let CentroidMethod::MoffatFit { beta } = self {
            InvalidConfigField::finite("Moffat beta", "finite and in (0, 10]", *beta, |value| {
                value > 0.0 && value <= 10.0
            })?;
        }
        Ok(())
    }
}

/// Method for computing local background during centroid refinement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LocalBackgroundMethod {
    /// Use the global background map (default, fastest).
    #[default]
    GlobalMap,
    /// Compute local background using an annular region around the star.
    /// Inner radius is based on stamp_radius, outer radius is 1.5× that.
    /// More accurate in regions with variable nebulosity.
    LocalAnnulus,
}

/// Sensor noise model for normalized linear pixels.
///
/// Lumos stores pixels in normalized units, so the conversion factor is electrons represented by
/// a pixel value of `1.0`, not the camera's electrons-per-ADU gain. Convert a physical gain with
/// `electrons_per_normalized_unit = electrons_per_adu * adu_per_normalized_unit`.
#[derive(Debug, Clone, Copy)]
pub struct NoiseModel {
    /// Electrons represented by one normalized pixel unit.
    pub electrons_per_normalized_unit: f32,
    /// Per-pixel read-noise standard deviation in electrons.
    pub read_noise_electrons: f32,
}

impl NoiseModel {
    /// Create a noise model whose signal scale already matches normalized Lumos pixels.
    pub fn from_normalized(electrons_per_normalized_unit: f32, read_noise_electrons: f32) -> Self {
        Self {
            electrons_per_normalized_unit,
            read_noise_electrons,
        }
    }

    /// Variance of an integrated normalized signal.
    ///
    /// `signal` is the summed background-subtracted signal, `background_noise` is the empirical
    /// per-pixel background standard deviation, and `sample_count` is the number of summed pixels.
    pub(crate) fn variance_normalized(
        &self,
        signal: f64,
        background_noise: f64,
        sample_count: usize,
    ) -> f64 {
        debug_assert!(signal.is_finite() && signal >= 0.0);
        debug_assert!(background_noise.is_finite() && background_noise >= 0.0);

        let electrons_per_unit = self.electrons_per_normalized_unit as f64;
        let read_noise_normalized = self.read_noise_electrons as f64 / electrons_per_unit;
        signal / electrons_per_unit
            + sample_count as f64
                * (background_noise * background_noise
                    + read_noise_normalized * read_noise_normalized)
    }

    /// Validate the noise model.
    pub fn validate(&self) -> Result<(), InvalidConfigField> {
        InvalidConfigField::finite(
            "electrons_per_normalized_unit",
            "finite and positive",
            self.electrons_per_normalized_unit,
            |value| value > 0.0,
        )?;
        InvalidConfigField::finite(
            "read_noise_electrons",
            "finite and non-negative",
            self.read_noise_electrons,
            |value| value >= 0.0,
        )?;
        Ok(())
    }
}

/// Configuration for centroid refinement and metric measurement.
#[derive(Debug, Clone)]
pub struct MeasurementConfig {
    /// Centroid refinement algorithm.
    pub centroid_method: CentroidMethod,
    /// Background source used for per-star measurement.
    pub local_background: LocalBackgroundMethod,
    /// Optional sensor model for variance-weighted fitting and SNR.
    pub noise_model: Option<NoiseModel>,
}

impl Default for MeasurementConfig {
    fn default() -> Self {
        Self {
            centroid_method: CentroidMethod::WeightedMoments,
            local_background: LocalBackgroundMethod::GlobalMap,
            noise_model: None,
        }
    }
}

impl MeasurementConfig {
    pub(super) fn validate(&self) -> Result<(), InvalidConfigField> {
        self.centroid_method.validate()?;
        if let Some(noise) = &self.noise_model {
            noise.validate()?;
        }
        Ok(())
    }
}
