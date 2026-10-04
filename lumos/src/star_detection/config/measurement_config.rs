//! Centroid and metric measurement settings.
//!
//! How to centroid, how to take a local background, and the electrons a unit of the samples holds.

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
    /// High precision (~0.01 pixel accuracy) but ~8x slower than `WeightedMoments`.
    /// Best for well-sampled, symmetric PSFs.
    GaussianFit,

    /// 2D Moffat profile fitting with configurable beta parameter.
    /// High precision (~0.01 pixel accuracy), similar speed to `GaussianFit`.
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
    pub(super) fn validate(self) -> Result<(), InvalidConfigField> {
        if let CentroidMethod::MoffatFit { beta } = self {
            InvalidConfigField::finite("Moffat beta", "finite and in (0, 10]", beta, |value| {
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
    /// Compute local background using an annulus around the star, from where a β = 2.5 Moffat of
    /// the expected FWHM holds 99% of its flux, out by a stamp radius. More accurate in regions
    /// with variable nebulosity.
    LocalAnnulus,
}

/// Configuration for centroid refinement and metric measurement.
#[derive(Debug, Clone)]
pub struct MeasurementConfig {
    /// Centroid refinement algorithm.
    pub centroid_method: CentroidMethod,
    /// Background source used for per-star measurement.
    pub local_background: LocalBackgroundMethod,
    /// Electrons per normalized unit of the samples, for the photon noise of the star's own light
    /// in the SNR and the fit weights. A pixel value of `1.0` holds this many electrons: a physical
    /// gain converts as `electrons_per_adu · adu_per_normalized_unit`. `None` takes the noise to be
    /// the measured background's alone. The read noise is not given apart: the background's σ,
    /// measured on the frame, already holds it, and Merline & Howell add it to the sky's shot
    /// noise, never to a measured σ.
    pub electrons_per_unit: Option<f32>,
}

impl Default for MeasurementConfig {
    fn default() -> Self {
        Self {
            centroid_method: CentroidMethod::WeightedMoments,
            local_background: LocalBackgroundMethod::GlobalMap,
            electrons_per_unit: None,
        }
    }
}

impl MeasurementConfig {
    pub(super) fn validate(&self) -> Result<(), InvalidConfigField> {
        self.centroid_method.validate()?;
        if let Some(electrons) = self.electrons_per_unit {
            InvalidConfigField::finite(
                "electrons_per_unit",
                "finite and positive",
                electrons,
                |value| value > 0.0,
            )?;
        }
        Ok(())
    }
}
