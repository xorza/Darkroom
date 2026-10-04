//! The noise model one cosmic-ray pass thresholds against, resolved for its frame.

use crate::background_mesh::colour_mesh::LocalBackground;
use crate::calibration_masters::cosmic_ray::config::NoiseEstimation;
use crate::calibration_masters::cosmic_ray::error::UnknownAdcStep;
use crate::io::image::cfa::QUANTIZATION_SIGMA_PER_STEP;
use crate::io::image::image_metadata::ImageMetadata;
use crate::io::image::sample_domain::ScaleOrigin;
use crate::math::noise::ccd_noise::CcdNoise;
use crate::math::statistics::spread::Spread;

/// [`NoiseEstimation`] resolved on a frame: the electrons one unit of its samples is worth, when
/// known.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct NoiseModel {
    electrons_per_unit: Option<f32>,
}

impl NoiseModel {
    pub(crate) fn resolve(
        estimation: &NoiseEstimation,
        metadata: &ImageMetadata,
    ) -> Result<Self, UnknownAdcStep> {
        let electrons_per_unit = match *estimation {
            NoiseEstimation::Measured => CcdNoise::electrons_per_unit(metadata),
            NoiseEstimation::Gain { electrons_per_adu } => {
                let adu_per_unit = match &metadata.domain {
                    Some(domain) if domain.origin == ScaleOrigin::Declared => domain.scale as f32,
                    _ => {
                        QUANTIZATION_SIGMA_PER_STEP
                            / metadata.quantization_sigma.ok_or(UnknownAdcStep)?
                    }
                };
                Some(electrons_per_adu * adu_per_unit)
            }
        };
        Ok(Self { electrons_per_unit })
    }

    /// The noise of a pixel whose median-filtered signal is `signal`, against the local background
    /// of its colour: `√(σ² + max(signal − sky, 0)·k)`, see [`NoiseEstimation`]. σ is floored at
    /// one `f32` step at the sky, so the significance never divides by zero.
    pub(crate) fn noise(self, signal: f32, local: LocalBackground) -> f32 {
        let sigma = local.noise.max(Spread::resolution(local.sky));
        let variance = sigma * sigma;
        let per_unit = match self.electrons_per_unit {
            Some(electrons) => 1.0 / electrons,
            None => variance / local.sky.max(sigma),
        };
        (variance + (signal - local.sky).max(0.0) * per_unit).sqrt()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::image::sample_domain::{Pedestal, SampleDomain};

    /// With a gain the source term is `(signal − sky)/electrons`: σ 0.5, sky 1, 4 electrons per
    /// unit, signal 3 gives √(0.25 + 2/4) = √0.75. Below the sky only σ counts. Without a gain the
    /// slope is σ²/max(sky, σ) = 0.25: √(0.25 + 2·0.25) as well. A σ of 0 is floored at one step at
    /// the sky, 2⁻²³.
    #[test]
    fn the_noise_adds_the_photons_above_the_local_sky() {
        let local = LocalBackground {
            sky: 1.0,
            noise: 0.5,
        };
        let gain = NoiseModel {
            electrons_per_unit: Some(4.0),
        };
        assert_eq!(gain.noise(3.0, local), 0.75f32.sqrt());
        assert_eq!(gain.noise(0.5, local), 0.5);
        let measured = NoiseModel {
            electrons_per_unit: None,
        };
        assert_eq!(measured.noise(3.0, local), 0.75f32.sqrt());
        let flat = LocalBackground {
            sky: 1.0,
            noise: 0.0,
        };
        assert_eq!(gain.noise(1.0, flat), f32::EPSILON);
    }

    /// A stated gain of 1.5 e⁻/ADU over a declared 4095 ADU per unit is 6142.5 electrons per unit;
    /// without a declared scale the quantization σ (1/√12)/4095 gives the same 4095 ADU, up to the
    /// two roundings of its quotient; with neither, the gain has no unit to apply to. The measured
    /// model reads the frame's own EGAIN.
    #[test]
    fn a_stated_gain_takes_its_unit_from_the_frame() {
        let estimation = NoiseEstimation::Gain {
            electrons_per_adu: 1.5,
        };
        let declared = ImageMetadata {
            domain: Some(SampleDomain {
                scale: 4095.0,
                origin: ScaleOrigin::Declared,
                pedestal: Pedestal::Removed,
                unit: None,
            }),
            ..Default::default()
        };
        assert_eq!(
            NoiseModel::resolve(&estimation, &declared)
                .unwrap()
                .electrons_per_unit,
            Some(6142.5)
        );
        let stepped = ImageMetadata {
            quantization_sigma: Some(QUANTIZATION_SIGMA_PER_STEP / 4095.0),
            ..Default::default()
        };
        assert_eq!(
            NoiseModel::resolve(&estimation, &stepped)
                .unwrap()
                .electrons_per_unit,
            Some(1.5 * (QUANTIZATION_SIGMA_PER_STEP / (QUANTIZATION_SIGMA_PER_STEP / 4095.0)))
        );
        assert_eq!(
            NoiseModel::resolve(&estimation, &ImageMetadata::default()),
            Err(UnknownAdcStep)
        );
        let egain = ImageMetadata {
            egain: Some(2.0),
            ..declared
        };
        assert_eq!(
            NoiseModel::resolve(&NoiseEstimation::Measured, &egain)
                .unwrap()
                .electrons_per_unit,
            Some(8190.0)
        );
    }
}
