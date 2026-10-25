//! The instrument + sensor model: PSF, charge capacity & noise, color filters, flat field, sensor
//! defects, and bias. A [`Camera`] turns true sky flux into raw sensor pixels.
//!
//! Pixel values are normalized flux where `1.0` == sensor full well; `full_well_e` is the
//! electron count at that level and sets the Poisson shot-noise scale (see
//! [`noise`](crate::internals::synthetic::noise)).

use std::f32::consts::PI;

use common::Buffer2;
use glam::Vec2;

use crate::image::cfa::cfa_type::CfaType;
use crate::internals::synthetic::bgs::Vignette;
use crate::internals::synthetic::star_profiles::{StarProfile, SyntheticStar};
use crate::math::fwhm;
use common::{Size2us, Vec2us};

/// Point-spread function the camera convolves every point source with.
#[derive(Debug, Clone, Copy)]
pub(crate) enum PsfModel {
    /// Gaussian of round-equivalent `fwhm` in pixels, elongated by `eccentricity` ∈ [0, 1) along
    /// a major axis at `angle` radians, as tracking error leaves a star; round at eccentricity 0
    /// ([`Self::gaussian`]).
    Gaussian {
        fwhm: f32,
        eccentricity: f32,
        angle: f32,
    },
    /// Moffat profile (extended atmospheric wings), `fwhm` in pixels, shape `beta`.
    Moffat { fwhm: f32, beta: f32 },
}

impl PsfModel {
    /// The round Gaussian of `fwhm` pixels.
    pub(crate) const fn gaussian(fwhm: f32) -> Self {
        Self::Gaussian {
            fwhm,
            eccentricity: 0.0,
            angle: 0.0,
        }
    }

    /// The round-equivalent FWHM (in pixels) a detector should recover.
    pub(crate) const fn fwhm(&self) -> f32 {
        match self {
            Self::Gaussian { fwhm, .. } | Self::Moffat { fwhm, .. } => *fwhm,
        }
    }

    /// This PSF under seeing that scales its width by `scale` (1.0 == nominal).
    pub(crate) const fn with_seeing(self, scale: f32) -> Self {
        match self {
            Self::Gaussian {
                fwhm,
                eccentricity,
                angle,
            } => Self::Gaussian {
                fwhm: fwhm * scale,
                eccentricity,
                angle,
            },
            Self::Moffat { fwhm, beta } => Self::Moffat {
                fwhm: fwhm * scale,
                beta,
            },
        }
    }

    /// Render a source of total `flux` centred at `centre` into `pixels`. Amplitudes are
    /// normalized so the rendered profile integrates to `flux` (flux is conserved up to the
    /// kernel's radius truncation).
    pub(crate) fn render(&self, pixels: &mut Buffer2<f32>, centre: Vec2, flux: f32) {
        let (profile, amplitude) = match *self {
            Self::Moffat { fwhm, beta } => {
                let alpha = fwhm::to_alpha(f64::from(fwhm), f64::from(beta)) as f32;
                (
                    StarProfile::Moffat { alpha, beta },
                    flux * (beta - 1.0) / (PI * alpha * alpha),
                )
            }
            Self::Gaussian {
                fwhm,
                eccentricity,
                angle,
            } => {
                // σ_maj·σ_min == σ² so total flux is independent of eccentricity.
                let sigma = fwhm::to_sigma(fwhm);
                let one_minus_e2 = 1.0 - eccentricity * eccentricity;
                let sigma_major = sigma / one_minus_e2.sqrt().sqrt();
                let sigma_minor = sigma_major * one_minus_e2.sqrt();
                (
                    StarProfile::Gaussian {
                        sigma_x: sigma_major,
                        sigma_y: sigma_minor,
                        angle,
                    },
                    flux / (2.0 * PI * sigma_major * sigma_minor),
                )
            }
        };
        SyntheticStar::new(centre, amplitude, profile).add_to(pixels);
    }
}

/// A multiplicative flat field (sensor response): optional radial vignette × per-color gain.
#[derive(Debug, Clone)]
pub(crate) struct FlatField {
    /// Radial vignette multiplier, or `None` for a flat 1.0 response.
    pub(crate) vignette: Option<Vignette>,
    /// The gain of each color's filter, in [`CfaType::slot_at`]'s slots (1.0 == no shift). A
    /// mono sensor uses slot 0.
    pub(crate) color_gain: [f32; 3],
}

impl Default for FlatField {
    fn default() -> Self {
        Self {
            vignette: None,
            color_gain: [1.0; 3],
        }
    }
}

impl FlatField {
    /// The response of every photosite of a `size` sensor behind `cfa_type`'s filters: its
    /// color's gain times the vignette.
    pub(crate) fn render(&self, size: Size2us, cfa_type: &CfaType) -> Buffer2<f32> {
        Buffer2::from_fn(size, |x, y| {
            let gain = self.color_gain[cfa_type.slot_at(Vec2us::new(x, y))];
            self.vignette
                .map_or(gain, |vignette| gain * vignette.at(size, x, y))
        })
    }
}

/// Sensor defects baked into a frame: hot pixels (additive spikes) and dead pixels (no
/// response to light or dark current; they read the bias and the read noise). Coordinates are
/// `(x, y)`.
#[derive(Debug, Clone, Default)]
pub(crate) struct SensorDefects {
    /// `(x, y, excess)` — hot pixels add `excess` normalized counts.
    pub(crate) hot: Vec<(usize, usize, f32)>,
    /// `(x, y)` — dead pixels, with no response.
    pub(crate) dead: Vec<(usize, usize)>,
}

/// Bias structure: a constant pedestal plus optional anomalous columns.
#[derive(Debug, Clone, Default)]
pub(crate) struct BiasField {
    /// Constant additive pedestal (normalized).
    pub(crate) offset: f32,
    /// `(col_x, excess_offset)` — bad columns sit above the base bias.
    pub(crate) bad_cols: Vec<(usize, f32)>,
}

/// The instrument + sensor: PSF, charge capacity & noise, flat, defects, bias.
#[derive(Debug, Clone)]
pub(crate) struct Camera {
    pub(crate) psf: PsfModel,
    /// Electrons at normalized value 1.0 (full well); sets the shot-noise scale. Inert when
    /// [`noiseless`](Self::noiseless) is set.
    pub(crate) full_well_e: f32,
    /// Read noise in electrons (Gaussian).
    pub(crate) read_noise_e: f32,
    /// Dark current in electrons per pixel per second (Poisson, × exposure).
    pub(crate) dark_current_e_per_s: f32,
    /// Saturation clip level in normalized units (typically 1.0).
    pub(crate) saturation: f32,
    /// The color filters over the photosites: `Mono` for none.
    pub(crate) cfa_type: CfaType,
    pub(crate) flat: FlatField,
    pub(crate) defects: SensorDefects,
    pub(crate) bias: BiasField,
    /// When set, render emits the clean signal — the stochastic layers (shot/dark/read) are
    /// skipped, so the frame *is* its own ground truth.
    pub(crate) noiseless: bool,
}

impl Camera {
    /// A noiseless camera (Gaussian PSF, no read/dark/shot noise, unit flat, no defects or
    /// bias). Rendering a scene through it yields the clean ground-truth image.
    pub(crate) fn ideal(fwhm: f32) -> Self {
        Self {
            psf: PsfModel::gaussian(fwhm),
            full_well_e: 50_000.0,
            read_noise_e: 0.0,
            dark_current_e_per_s: 0.0,
            saturation: 1.0,
            cfa_type: CfaType::Mono,
            flat: FlatField::default(),
            defects: SensorDefects::default(),
            bias: BiasField::default(),
            noiseless: true,
        }
    }

    /// A representative cooled-CMOS camera: 50 ke⁻ well, 3 e⁻ read noise, low dark current.
    pub(crate) fn realistic(fwhm: f32) -> Self {
        Self {
            psf: PsfModel::gaussian(fwhm),
            full_well_e: 50_000.0,
            read_noise_e: 3.0,
            dark_current_e_per_s: 0.05,
            saturation: 1.0,
            cfa_type: CfaType::Mono,
            flat: FlatField::default(),
            defects: SensorDefects::default(),
            bias: BiasField::default(),
            noiseless: false,
        }
    }
}

#[cfg(test)]
mod tests;
