//! The noise model one cosmic-ray pass thresholds against, resolved for its frame.

use common::USize2;

use crate::bg_mesh::color_mesh::{ColorMesh, LocalBg};
use crate::calibration::cosmic_ray::cosmic_ray_config::NoiseEstimation;
use crate::calibration::cosmic_ray::error::CosmicRayError;
use crate::image::cfa::bayer_phase::BayerPhase;
use crate::image::flat_gain::FlatGain;
use crate::image::metadata::image_metadata::ImageMetadata;
use crate::image::metadata::sample_domain::Pedestal;
use crate::math::statistics::spread::Spread;
use crate::noise::QUANTIZATION_SIGMA_PER_STEP;
use crate::noise::ccd_noise::NoiseAt;
use glam::USizeVec2;

/// [`NoiseEstimation`] resolved on a frame: the electrons one unit of its samples is worth, or,
/// without them, the level of zero signal the sky stands on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum NoiseModel {
    /// The gain: a unit above the sky adds `flat_gain/electrons` of variance.
    Gain { electrons_per_unit: f32 },
    /// No gain: the background σ stands in, the sky above `pedestal` taken as photon-limited.
    Sky { pedestal: f32 },
}

impl NoiseModel {
    pub(crate) fn resolve(
        estimation: &NoiseEstimation,
        metadata: &ImageMetadata,
    ) -> Result<Self, CosmicRayError> {
        match *estimation {
            NoiseEstimation::Measured => Ok(match metadata.electrons_per_unit() {
                Some(electrons_per_unit) => Self::Gain { electrons_per_unit },
                None => Self::Sky {
                    pedestal: Self::pedestal(metadata)?,
                },
            }),
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
                Ok(Self::Gain {
                    electrons_per_unit: electrons_per_adu * adu_per_unit,
                })
            }
        }
    }

    /// The level of zero signal in `metadata`'s frame, in its sample units: a kept pedestal over
    /// the domain's scale, 0 for a removed one, a bias-calibrated frame or one of no declared
    /// domain, whose values a caller made from zero.
    ///
    /// # Errors
    ///
    /// [`CosmicRayError::UnknownPedestal`] for an undeclared pedestal no bias removed.
    fn pedestal(metadata: &ImageMetadata) -> Result<f32, CosmicRayError> {
        let Some(domain) = metadata.derived.domain() else {
            return Ok(0.0);
        };
        match domain.pedestal {
            Pedestal::Removed => Ok(0.0),
            Pedestal::Kept(level) => Ok((level / domain.scale) as f32),
            Pedestal::Unknown if metadata.derived.calibration().bias => Ok(0.0),
            Pedestal::Unknown => Err(CosmicRayError::UnknownPedestal),
        }
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
    /// the variance a unit adds were the sky photon-limited, `σ² = sky·g/electrons`, with the sky
    /// taken above the pedestal its values still carry, which holds no photons. Measured at the
    /// pixel's flat, it holds `g` already; read noise in σ² only raises it, which raises the noise
    /// and flags fewer pixels, the side a guess has to err on.
    pub(crate) fn noise(self, signal: f32, pixel: PixelBg) -> f32 {
        let local = pixel.local;
        let sigma = local.noise.max(Spread::resolution(local.sky));
        let variance = sigma * sigma;
        NoiseAt {
            bg: variance,
            sky: local.sky,
            per_unit: match self {
                Self::Gain { electrons_per_unit } => pixel.flat_gain / electrons_per_unit,
                Self::Sky { pedestal } => variance / (local.sky - pedestal).max(sigma),
            },
        }
        .variance(signal)
        .sqrt()
    }
}

/// What the noise model reads at one pixel: its color's local background, and the gain a flat
/// multiplied it by.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PixelBg {
    pub(crate) local: LocalBg,
    pub(crate) flat_gain: f32,
}

/// The [`PixelBg`] of every pixel of a frame: its color mesh, and its flat's gain.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PixelBgs<'a> {
    pub(crate) mesh: &'a ColorMesh,
    /// `None` for a frame no flat divided, whose gain is 1 everywhere.
    pub(crate) flat_gain: Option<&'a FlatGain>,
}

impl PixelBgs<'_> {
    pub(crate) fn at(&self, color: usize, pos: USizeVec2) -> PixelBg {
        PixelBg {
            local: self.mesh.at(color, pos),
            flat_gain: self
                .flat_gain
                .map_or(1.0, |gain| gain.at(color, pos.x as f32, pos.y as f32)),
        }
    }
}

/// Where a detection reads each pixel's background, by flat index into the plane it scans: a whole
/// plane of the mesh's color 0, or one Bayer phase of a mosaic, of its color.
#[derive(Debug, Clone, Copy)]
pub(crate) enum PixelBgSource<'a> {
    Plane {
        bgs: PixelBgs<'a>,
        size: USize2,
    },
    Phase {
        bgs: PixelBgs<'a>,
        color: usize,
        phase: BayerPhase,
        size: USize2,
    },
    /// One background at every pixel, for a test to scan against.
    #[cfg(test)]
    Flat(PixelBg),
}

impl PixelBgSource<'_> {
    /// The background of pixel `idx` of the plane scanned.
    pub(crate) fn at(self, idx: usize) -> PixelBg {
        match self {
            Self::Plane { bgs, size } => bgs.at(0, size.point_of(idx)),
            Self::Phase {
                bgs,
                color,
                phase,
                size,
            } => bgs.at(color, phase.to_mosaic(size.point_of(idx))),
            #[cfg(test)]
            Self::Flat(bg) => bg,
        }
    }
}

#[cfg(test)]
mod tests;
