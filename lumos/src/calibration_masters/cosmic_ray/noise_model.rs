//! The noise model one cosmic-ray pass thresholds against, resolved for its frame.

use crate::background_mesh::colour_mesh::{ColourMesh, LocalBackground};
use crate::calibration_masters::cosmic_ray::config::NoiseEstimation;
use crate::calibration_masters::cosmic_ray::error::UnknownAdcStep;
use crate::io::image::cfa::QUANTIZATION_SIGMA_PER_STEP;
use crate::io::image::flat_gain::FlatGain;
use crate::io::image::image_metadata::ImageMetadata;
use crate::io::image::sample_domain::ScaleOrigin;
use crate::math::noise::ccd_noise::CcdNoise;
use crate::math::statistics::spread::Spread;
use crate::math::vec2us::Vec2us;

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

    /// The noise of a pixel whose median-filtered signal is `signal`, against `pixel`'s local
    /// background: `√(σ² + max(signal − sky, 0)·k)`, see [`NoiseEstimation`]. With a gain,
    /// `k = g/electrons`: the flat multiplied the photons' count by `g`, and with it their
    /// variance per unit. Without one, `k = σ²/sky` is measured at the pixel's own flat, so it
    /// already holds `g`. σ is floored at one `f32` step at the sky, so the significance never
    /// divides by zero.
    pub(crate) fn noise(self, signal: f32, pixel: PixelBackground) -> f32 {
        let local = pixel.local;
        let sigma = local.noise.max(Spread::resolution(local.sky));
        let variance = sigma * sigma;
        let per_unit = match self.electrons_per_unit {
            Some(electrons) => pixel.flat_gain / electrons,
            None => variance / local.sky.max(sigma),
        };
        (variance + (signal - local.sky).max(0.0) * per_unit).sqrt()
    }
}

/// What the noise model reads at one pixel: its colour's local background, and the gain a flat
/// multiplied it by.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PixelBackground {
    pub(crate) local: LocalBackground,
    pub(crate) flat_gain: f32,
}

/// The [`PixelBackground`] of every pixel of a frame: its colour mesh, and its flat's gain.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PixelBackgrounds<'a> {
    pub(crate) mesh: &'a ColourMesh,
    /// `None` for a frame no flat divided, whose gain is 1 everywhere.
    pub(crate) flat_gain: Option<&'a FlatGain>,
}

impl PixelBackgrounds<'_> {
    pub(crate) fn at(&self, colour: usize, position: Vec2us) -> PixelBackground {
        PixelBackground {
            local: self.mesh.at(colour, position),
            flat_gain: self.flat_gain.map_or(1.0, |gain| {
                gain.at(colour, position.x as f32, position.y as f32)
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use imaginarium::Buffer2;

    use super::*;
    use crate::background_mesh::workspace::MeshWorkspace;
    use crate::io::image::cfa::CfaType;
    use crate::io::image::sample_domain::{Pedestal, SampleDomain};
    use crate::math::size2us::Size2us;

    /// With a gain the source term is `(signal − sky)·g/electrons`: σ 0.5, sky 1, 4 electrons per
    /// unit, signal 3 gives √(0.25 + 2/4) = √0.75 where no flat amplified the pixel, and
    /// √(0.25 + 2·2/4) = √1.25 where a flat doubled it. Below the sky only σ counts. Without a gain
    /// the slope is σ²/max(sky, σ) = 0.25, measured at the pixel's own flat: √(0.25 + 2·0.25) at
    /// either gain. A σ of 0 is floored at one step at the sky, 2⁻²³.
    #[test]
    fn the_noise_adds_the_photons_above_the_local_sky() {
        let pixel = |noise: f32, flat_gain: f32| PixelBackground {
            local: LocalBackground { sky: 1.0, noise },
            flat_gain,
        };
        let gain = NoiseModel {
            electrons_per_unit: Some(4.0),
        };
        assert_eq!(gain.noise(3.0, pixel(0.5, 1.0)), 0.75f32.sqrt());
        assert_eq!(gain.noise(3.0, pixel(0.5, 2.0)), 1.25f32.sqrt());
        assert_eq!(gain.noise(0.5, pixel(0.5, 2.0)), 0.5);
        let measured = NoiseModel {
            electrons_per_unit: None,
        };
        for flat_gain in [1.0, 2.0] {
            assert_eq!(measured.noise(3.0, pixel(0.5, flat_gain)), 0.75f32.sqrt());
        }
        assert_eq!(gain.noise(1.0, pixel(0.0, 1.0)), f32::EPSILON);
    }

    /// A pixel's background is its colour's mesh value and its flat's gain there: a frame no flat
    /// divided reads gain 1, one under a flat of 0.5 reads 2, the sky the mesh measured either way.
    #[test]
    fn a_pixel_reads_its_mesh_and_its_flat_gain() {
        let size = Size2us::new(16, 16);
        let mesh = ColourMesh::measure(
            &Buffer2::new(16, 16, vec![0.5; size.pixel_count()]),
            &CfaType::Mono,
            8,
            &mut MeshWorkspace::default(),
        );
        let gain = FlatGain::of_divisor(
            &Buffer2::new(16, 16, vec![0.5; 256]),
            &CfaType::Mono,
            |_| false,
        );
        for (flat_gain, expected) in [(None, 1.0), (Some(&gain), 2.0)] {
            let pixel = PixelBackgrounds {
                mesh: &mesh,
                flat_gain,
            }
            .at(0, Vec2us::new(5, 9));
            assert_eq!(pixel.flat_gain, expected);
            assert_eq!(pixel.local.sky, 0.5);
        }
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
