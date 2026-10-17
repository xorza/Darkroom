//! The noise model one cosmic-ray pass thresholds against, resolved for its frame.

use crate::background_mesh::color_mesh::{ColorMesh, LocalBackground};
use crate::calibration::cosmic_ray::cosmic_ray_config::NoiseEstimation;
use crate::calibration::cosmic_ray::error::CosmicRayError;
use crate::image::flat_gain::FlatGain;
use crate::image::image_metadata::ImageMetadata;
use crate::math::statistics::spread::Spread;
use crate::math::vec2us::Vec2us;
use crate::noise::QUANTIZATION_SIGMA_PER_STEP;
use crate::noise::ccd_noise::NoiseAt;

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
    ) -> Result<Self, CosmicRayError> {
        let electrons_per_unit = match *estimation {
            NoiseEstimation::Measured => metadata.electrons_per_unit(),
            NoiseEstimation::Gain { electrons_per_adu } => {
                let adu_per_unit = match metadata.derived.adu_per_unit() {
                    Some(adu_per_unit) => adu_per_unit as f32,
                    None => {
                        QUANTIZATION_SIGMA_PER_STEP
                            / metadata
                                .derived
                                .quantization_sigma()
                                .ok_or(CosmicRayError::UnknownAdcStep)?
                    }
                };
                Some(electrons_per_adu * adu_per_unit)
            }
        };
        Ok(Self { electrons_per_unit })
    }

    /// The noise of a pixel whose median-filtered signal is `signal`, against `pixel`'s local
    /// background: the CCD equation ([`NoiseAt`]) on the local σ² and sky, see
    /// [`NoiseEstimation`]. σ is measured at the pixel's own flat, so the background is not
    /// carried through it again, and is floored at one `f32` step at the sky, so the significance
    /// never divides by zero.
    ///
    /// With a gain, a unit above the sky adds `k = g/electrons`: the flat multiplied the photons'
    /// count by `g`, and with it their variance per unit. Without one, the CCD model has no source
    /// term, but a detector without one flags every star's bright core: `k = σ²/sky` stands in,
    /// the variance a unit adds were the sky photon-limited, `σ² = sky·g/electrons`. Measured at
    /// the pixel's flat, it holds `g` already; read noise in σ² only raises it, which raises the
    /// noise and flags fewer pixels, the side a guess has to err on.
    pub(crate) fn noise(self, signal: f32, pixel: PixelBackground) -> f32 {
        let local = pixel.local;
        let sigma = local.noise.max(Spread::resolution(local.sky));
        let variance = sigma * sigma;
        NoiseAt {
            background: variance,
            sky: local.sky,
            per_unit: match self.electrons_per_unit {
                Some(electrons) => pixel.flat_gain / electrons,
                None => variance / local.sky.max(sigma),
            },
        }
        .variance(signal)
        .sqrt()
    }
}

/// What the noise model reads at one pixel: its color's local background, and the gain a flat
/// multiplied it by.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PixelBackground {
    pub(crate) local: LocalBackground,
    pub(crate) flat_gain: f32,
}

/// The [`PixelBackground`] of every pixel of a frame: its color mesh, and its flat's gain.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PixelBackgrounds<'a> {
    pub(crate) mesh: &'a ColorMesh,
    /// `None` for a frame no flat divided, whose gain is 1 everywhere.
    pub(crate) flat_gain: Option<&'a FlatGain>,
}

impl PixelBackgrounds<'_> {
    pub(crate) fn at(&self, color: usize, position: Vec2us) -> PixelBackground {
        PixelBackground {
            local: self.mesh.at(color, position),
            flat_gain: self.flat_gain.map_or(1.0, |gain| {
                gain.at(color, position.x as f32, position.y as f32)
            }),
        }
    }
}

#[cfg(test)]
mod tests;
