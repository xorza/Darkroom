//! The noise model one cosmic-ray pass thresholds against, resolved for its frame.

use crate::background_mesh::colour_mesh::{ColourMesh, LocalBackground};
use crate::calibration::cosmic_ray::cosmic_ray_config::NoiseEstimation;
use crate::calibration::cosmic_ray::error::CosmicRayError;
use crate::image::cfa::QUANTIZATION_SIGMA_PER_STEP;
use crate::image::flat_gain::FlatGain;
use crate::image::image_metadata::ImageMetadata;
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
    ) -> Result<Self, CosmicRayError> {
        let electrons_per_unit = match *estimation {
            NoiseEstimation::Measured => metadata.electrons_per_unit(),
            NoiseEstimation::Gain { electrons_per_adu } => {
                let adu_per_unit = match metadata.adu_per_unit() {
                    Some(adu_per_unit) => adu_per_unit as f32,
                    None => {
                        QUANTIZATION_SIGMA_PER_STEP
                            / metadata
                                .quantization_sigma
                                .ok_or(CosmicRayError::UnknownAdcStep)?
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
mod tests;
