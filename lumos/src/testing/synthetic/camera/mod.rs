//! The instrument + sensor model: PSF, charge capacity & noise, flat field, sensor
//! defects, and bias. A [`Camera`] turns true sky flux into raw sensor pixels.
//!
//! Pixel values are normalized flux where `1.0` == sensor full well; `full_well_e` is the
//! electron count at that level and sets the Poisson shot-noise scale (see
//! [`noise`](crate::testing::synthetic::noise)).

use crate::math::fwhm::fwhm_to_sigma;
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;
use crate::testing::synthetic::star_profiles::{StarProfile, SyntheticStar, fwhm_to_moffat_alpha};
use glam::Vec2;
use imaginarium::Buffer2;
use std::f32::consts::PI;

/// Point-spread function the camera convolves every point source with.
#[derive(Debug, Clone, Copy)]
pub(crate) enum PsfModel {
    /// Circular Gaussian, `fwhm` in pixels.
    Gaussian { fwhm: f32 },
    /// Moffat profile (extended atmospheric wings), `fwhm` in pixels, shape `beta`.
    Moffat { fwhm: f32, beta: f32 },
    /// Elliptical Gaussian (tracking error): round-equivalent `fwhm`, `eccentricity`
    /// ∈ [0, 1), major-axis `angle` in radians.
    Elliptical {
        fwhm: f32,
        eccentricity: f32,
        angle: f32,
    },
}

impl PsfModel {
    /// The round-equivalent FWHM (in pixels) a detector should recover.
    pub(crate) fn fwhm(&self) -> f32 {
        match self {
            PsfModel::Gaussian { fwhm } => *fwhm,
            PsfModel::Moffat { fwhm, .. } => *fwhm,
            PsfModel::Elliptical { fwhm, .. } => *fwhm,
        }
    }

    /// Render a source of total `flux` centered at (`x`, `y`) into `pixels`, scaling the PSF
    /// width by `seeing_scale` (1.0 == nominal). Amplitudes are normalized so the rendered
    /// profile integrates to `flux` (flux is conserved up to the kernel's radius truncation).
    pub(crate) fn render(
        &self,
        pixels: &mut Buffer2<f32>,
        x: f32,
        y: f32,
        flux: f32,
        seeing_scale: f32,
    ) {
        let (profile, amplitude) = match *self {
            PsfModel::Gaussian { fwhm } => {
                let sigma = fwhm_to_sigma(fwhm * seeing_scale);
                (
                    StarProfile::Gaussian { sigma },
                    flux / (2.0 * PI * sigma * sigma),
                )
            }
            PsfModel::Moffat { fwhm, beta } => {
                let alpha = fwhm_to_moffat_alpha(fwhm * seeing_scale, beta);
                (
                    StarProfile::Moffat { alpha, beta },
                    flux * (beta - 1.0) / (PI * alpha * alpha),
                )
            }
            PsfModel::Elliptical {
                fwhm,
                eccentricity,
                angle,
            } => {
                // σ_maj·σ_min == σ² so total flux is independent of eccentricity.
                let sigma = fwhm_to_sigma(fwhm * seeing_scale);
                let one_minus_e2 = 1.0 - eccentricity * eccentricity;
                let sigma_major = sigma / one_minus_e2.sqrt().sqrt();
                let sigma_minor = sigma_major * one_minus_e2.sqrt();
                (
                    StarProfile::Elliptical {
                        sigma_x: sigma_major,
                        sigma_y: sigma_minor,
                        angle,
                    },
                    flux / (2.0 * PI * sigma_major * sigma_minor),
                )
            }
        };
        SyntheticStar::new(Vec2::new(x, y), amplitude, profile).add_to(pixels);
    }
}

/// A multiplicative flat field (sensor response): optional radial vignette × per-channel gain.
#[derive(Debug, Clone)]
pub(crate) struct FlatField {
    /// `(center, edge, falloff)` radial vignette multiplier, or `None` for a flat 1.0 response.
    pub(crate) vignette: Option<(f32, f32, f32)>,
    /// Per-RGB-channel multiplicative gain (1.0 == no shift). Mono uses index 0.
    pub(crate) channel_gain: [f32; 3],
}

impl Default for FlatField {
    fn default() -> Self {
        Self {
            vignette: None,
            channel_gain: [1.0; 3],
        }
    }
}

impl FlatField {
    /// Render the flat-field response map for `channel` into a fresh `size`-sized buffer.
    pub(crate) fn render(&self, size: Size2us, channel: usize) -> Vec<f32> {
        let gain = self.channel_gain[channel];
        let mut flat = vec![gain; size.pixel_count()];
        if let Some((center, edge, falloff)) = self.vignette {
            let cx = size.width as f32 / 2.0;
            let cy = size.height as f32 / 2.0;
            let max_r = (cx * cx + cy * cy).sqrt().max(1.0);
            for y in 0..size.height {
                for x in 0..size.width {
                    let dx = x as f32 - cx;
                    let dy = y as f32 - cy;
                    let t = ((dx * dx + dy * dy).sqrt() / max_r).powf(falloff);
                    flat[size.index_of(Vec2us::new(x, y))] = gain * (center + (edge - center) * t);
                }
            }
        }
        flat
    }
}

/// Sensor defects baked into a frame: hot pixels (additive spikes) and dead pixels
/// (forced to zero). Coordinates are `(x, y)`.
#[derive(Debug, Clone, Default)]
pub(crate) struct SensorDefects {
    /// `(x, y, excess)` — hot pixels add `excess` normalized counts.
    pub(crate) hot: Vec<(usize, usize, f32)>,
    /// `(x, y)` — dead pixels forced to ~zero response.
    pub(crate) dead: Vec<(usize, usize)>,
}

/// Bias structure: a constant pedestal plus optional anomalous columns.
#[derive(Debug, Clone, Default)]
pub(crate) struct BiasField {
    /// Constant additive pedestal (normalized).
    pub(crate) offset: f32,
    /// `(column_x, excess_offset)` — bad columns sit above the base bias.
    pub(crate) bad_columns: Vec<(usize, f32)>,
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
            psf: PsfModel::Gaussian { fwhm },
            full_well_e: 50_000.0,
            read_noise_e: 0.0,
            dark_current_e_per_s: 0.0,
            saturation: 1.0,
            flat: FlatField::default(),
            defects: SensorDefects::default(),
            bias: BiasField::default(),
            noiseless: true,
        }
    }

    /// A representative cooled-CMOS camera: 50 ke⁻ well, 3 e⁻ read noise, low dark current.
    pub(crate) fn realistic(fwhm: f32) -> Self {
        Self {
            psf: PsfModel::Gaussian { fwhm },
            full_well_e: 50_000.0,
            read_noise_e: 3.0,
            dark_current_e_per_s: 0.05,
            saturation: 1.0,
            flat: FlatField::default(),
            defects: SensorDefects::default(),
            bias: BiasField::default(),
            noiseless: false,
        }
    }
}

#[cfg(test)]
mod tests;
