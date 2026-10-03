//! The noise model one cosmic-ray pass thresholds against, resolved for its frame.

use crate::calibration_masters::cosmic_ray::config::NoiseEstimation;
use crate::calibration_masters::cosmic_ray::error::UnknownAdcStep;
use crate::io::image::cfa::QUANTIZATION_SIGMA_PER_STEP;

/// [`NoiseEstimation`] with what the frame supplies filled in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum NoiseModel {
    Empirical,
    /// `full_scale` is the ADU one sample unit is worth: one ADC step's σ, `1/√12` ADU, over the
    /// frame's quantization σ in sample units.
    Parametric {
        gain: f32,
        read_noise: f32,
        full_scale: f32,
    },
}

impl NoiseModel {
    /// The model `estimation` names, on a frame whose decoder recorded `quantization_sigma`.
    pub(crate) fn resolve(
        estimation: &NoiseEstimation,
        quantization_sigma: Option<f32>,
    ) -> Result<Self, UnknownAdcStep> {
        Ok(match *estimation {
            NoiseEstimation::Empirical => Self::Empirical,
            NoiseEstimation::Parametric { gain, read_noise } => Self::Parametric {
                gain,
                read_noise,
                full_scale: QUANTIZATION_SIGMA_PER_STEP
                    / quantization_sigma.ok_or(UnknownAdcStep)?,
            },
        })
    }
}
