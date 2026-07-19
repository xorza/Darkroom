//! The instrument + sensor model: PSF, charge capacity & noise, flat field, sensor
//! defects, and bias. A [`Camera`] turns true sky flux into raw sensor pixels.
//!
//! Pixel values are normalized flux where `1.0` == sensor full well; `full_well_e` is the
//! electron count at that level and sets the Poisson shot-noise scale (see
//! [`noise`](crate::testing::synthetic::noise)).

use crate::math::fwhm_to_sigma;
use crate::testing::synthetic::star_profiles::{
    fwhm_to_moffat_alpha, render_elliptical_star, render_gaussian_star, render_moffat_star,
};
use std::f32::consts::PI;

/// Point-spread function the camera convolves every point source with.
#[derive(Debug, Clone, Copy)]
pub enum PsfModel {
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
    pub fn fwhm(&self) -> f32 {
        match self {
            PsfModel::Gaussian { fwhm } => *fwhm,
            PsfModel::Moffat { fwhm, .. } => *fwhm,
            PsfModel::Elliptical { fwhm, .. } => *fwhm,
        }
    }

    /// Render a source of total `flux` centered at (`x`, `y`) into `pixels`, scaling the PSF
    /// width by `seeing_scale` (1.0 == nominal). Amplitudes are normalized so the rendered
    /// profile integrates to `flux` (flux is conserved up to the kernel's radius truncation).
    pub fn render(
        &self,
        pixels: &mut [f32],
        width: usize,
        x: f32,
        y: f32,
        flux: f32,
        seeing_scale: f32,
    ) {
        match *self {
            PsfModel::Gaussian { fwhm } => {
                let sigma = fwhm_to_sigma(fwhm * seeing_scale);
                let amplitude = flux / (2.0 * PI * sigma * sigma);
                render_gaussian_star(pixels, width, x, y, sigma, amplitude);
            }
            PsfModel::Moffat { fwhm, beta } => {
                let alpha = fwhm_to_moffat_alpha(fwhm * seeing_scale, beta);
                let amplitude = flux * (beta - 1.0) / (PI * alpha * alpha);
                render_moffat_star(pixels, width, x, y, alpha, beta, amplitude);
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
                let amplitude = flux / (2.0 * PI * sigma_major * sigma_minor);
                render_elliptical_star(
                    pixels,
                    width,
                    x,
                    y,
                    sigma_major,
                    sigma_minor,
                    angle,
                    amplitude,
                );
            }
        }
    }
}

/// A multiplicative flat field (sensor response): optional radial vignette × per-channel gain.
#[derive(Debug, Clone)]
pub struct FlatField {
    /// `(center, edge, falloff)` radial vignette multiplier, or `None` for a flat 1.0 response.
    pub vignette: Option<(f32, f32, f32)>,
    /// Per-RGB-channel multiplicative gain (1.0 == no shift). Mono uses index 0.
    pub channel_gain: [f32; 3],
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
    /// Render the flat-field response map for `channel` into a fresh `width*height` buffer.
    pub fn render(&self, width: usize, height: usize, channel: usize) -> Vec<f32> {
        let gain = self.channel_gain[channel];
        let mut flat = vec![gain; width * height];
        if let Some((center, edge, falloff)) = self.vignette {
            let cx = width as f32 / 2.0;
            let cy = height as f32 / 2.0;
            let max_r = (cx * cx + cy * cy).sqrt().max(1.0);
            for y in 0..height {
                for x in 0..width {
                    let dx = x as f32 - cx;
                    let dy = y as f32 - cy;
                    let t = ((dx * dx + dy * dy).sqrt() / max_r).powf(falloff);
                    flat[y * width + x] = gain * (center + (edge - center) * t);
                }
            }
        }
        flat
    }
}

/// Sensor defects baked into a frame: hot pixels (additive spikes) and dead pixels
/// (forced to zero). Coordinates are `(x, y)`.
#[derive(Debug, Clone, Default)]
pub struct SensorDefects {
    /// `(x, y, excess)` — hot pixels add `excess` normalized counts.
    pub hot: Vec<(usize, usize, f32)>,
    /// `(x, y)` — dead pixels forced to ~zero response.
    pub dead: Vec<(usize, usize)>,
}

/// Bias structure: a constant pedestal plus optional anomalous columns.
#[derive(Debug, Clone, Default)]
pub struct BiasField {
    /// Constant additive pedestal (normalized).
    pub offset: f32,
    /// `(column_x, excess_offset)` — bad columns sit above the base bias.
    pub bad_columns: Vec<(usize, f32)>,
}

/// The instrument + sensor: PSF, charge capacity & noise, flat, defects, bias.
#[derive(Debug, Clone)]
pub struct Camera {
    pub psf: PsfModel,
    /// Electrons at normalized value 1.0 (full well); sets the shot-noise scale. Inert when
    /// [`noiseless`](Self::noiseless) is set.
    pub full_well_e: f32,
    /// Read noise in electrons (Gaussian).
    pub read_noise_e: f32,
    /// Dark current in electrons per pixel per second (Poisson, × exposure).
    pub dark_current_e_per_s: f32,
    /// Saturation clip level in normalized units (typically 1.0).
    pub saturation: f32,
    pub flat: FlatField,
    pub defects: SensorDefects,
    pub bias: BiasField,
    /// When set, render emits the clean signal — the stochastic layers (shot/dark/read) are
    /// skipped, so the frame *is* its own ground truth.
    pub noiseless: bool,
}

impl Camera {
    /// A noiseless camera (Gaussian PSF, no read/dark/shot noise, unit flat, no defects or
    /// bias). Rendering a scene through it yields the clean ground-truth image.
    pub fn ideal(fwhm: f32) -> Self {
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
    pub fn realistic(fwhm: f32) -> Self {
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
mod tests {
    use crate::testing::synthetic::camera::*;

    fn render_one(psf: PsfModel, size: usize, flux: f32, seeing: f32) -> Vec<f32> {
        let mut pixels = vec![0.0f32; size * size];
        let c = size as f32 / 2.0;
        psf.render(&mut pixels, size, c, c, flux, seeing);
        pixels
    }

    #[test]
    fn gaussian_psf_conserves_flux() {
        // A wide canvas captures essentially all of the 4σ-truncated profile.
        let pixels = render_one(PsfModel::Gaussian { fwhm: 4.0 }, 81, 100.0, 1.0);
        let sum: f32 = pixels.iter().sum();
        assert!((sum - 100.0).abs() < 1.0, "sum {sum}");
    }

    #[test]
    fn moffat_psf_conserves_flux() {
        // β=2.5, rendered to 8α: enclosed fraction 1-(1+64)^-1.5 ≈ 0.998.
        let pixels = render_one(
            PsfModel::Moffat {
                fwhm: 4.0,
                beta: 2.5,
            },
            121,
            100.0,
            1.0,
        );
        let sum: f32 = pixels.iter().sum();
        assert!((sum - 100.0).abs() < 2.0, "sum {sum}");
    }

    #[test]
    fn elliptical_psf_conserves_flux_and_is_elongated() {
        let psf = PsfModel::Elliptical {
            fwhm: 4.0,
            eccentricity: 0.6,
            angle: 0.0,
        };
        let size = 81;
        let pixels = render_one(psf, size, 100.0, 1.0);
        let sum: f32 = pixels.iter().sum();
        assert!((sum - 100.0).abs() < 1.5, "sum {sum}");
        // Major axis horizontal (angle 0): more flux 6px right of center than 6px below.
        let c = size / 2;
        let horiz = pixels[c * size + (c + 6)];
        let vert = pixels[(c + 6) * size + c];
        assert!(horiz > vert, "horiz {horiz} vert {vert}");
    }

    #[test]
    fn seeing_scale_widens_and_lowers_peak() {
        let psf = PsfModel::Gaussian { fwhm: 4.0 };
        let size = 81;
        let c = size / 2;
        let sharp = render_one(psf, size, 100.0, 1.0);
        let blurred = render_one(psf, size, 100.0, 2.0);
        // Same flux spread over a wider PSF → lower peak, but flux still conserved.
        assert!(blurred[c * size + c] < sharp[c * size + c]);
        let (s_sum, b_sum): (f32, f32) = (sharp.iter().sum(), blurred.iter().sum());
        assert!((s_sum - b_sum).abs() < 2.0);
    }

    #[test]
    fn flat_field_default_is_unit() {
        let flat = FlatField::default().render(8, 8, 0);
        assert!(flat.iter().all(|&f| (f - 1.0).abs() < 1e-6));
    }

    #[test]
    fn flat_field_channel_gain_and_vignette() {
        // Per-channel gain scales the whole map.
        let ff = FlatField {
            vignette: None,
            channel_gain: [0.9, 1.0, 1.1],
        };
        assert!((ff.render(4, 4, 2)[0] - 1.1).abs() < 1e-6);

        // Vignette: center brighter than corner.
        let vig = FlatField {
            vignette: Some((1.0, 0.5, 2.0)),
            channel_gain: [1.0; 3],
        };
        let map = vig.render(64, 64, 0);
        let center = map[32 * 64 + 32];
        let corner = map[0];
        assert!(center > corner, "center {center} corner {corner}");
        assert!((center - 1.0).abs() < 0.05);
    }

    #[test]
    fn psf_fwhm_accessor() {
        assert_eq!(PsfModel::Gaussian { fwhm: 3.5 }.fwhm(), 3.5);
        assert_eq!(
            PsfModel::Moffat {
                fwhm: 4.0,
                beta: 3.0
            }
            .fwhm(),
            4.0
        );
    }

    #[test]
    fn moffat_has_heavier_wings_than_gaussian() {
        // Equal FWHM and flux: the Gaussian concentrates flux in the core, the Moffat spreads it
        // into atmospheric wings. At r=10 px the Gaussian (4σ-truncated) is gone; the Moffat is not.
        let size = 121;
        let c = size / 2;
        let g = render_one(PsfModel::Gaussian { fwhm: 4.0 }, size, 100.0, 1.0);
        let m = render_one(
            PsfModel::Moffat {
                fwhm: 4.0,
                beta: 2.5,
            },
            size,
            100.0,
            1.0,
        );
        assert!(
            g[c * size + c] > m[c * size + c],
            "Gaussian core {} should exceed Moffat core {}",
            g[c * size + c],
            m[c * size + c]
        );
        assert!(
            m[c * size + (c + 10)] > 0.005,
            "Moffat should carry real wing flux at r=10, got {}",
            m[c * size + (c + 10)]
        );
        assert!(
            g[c * size + (c + 10)] < 1e-4,
            "Gaussian wings should be negligible at r=10, got {}",
            g[c * size + (c + 10)]
        );
    }

    #[test]
    fn moffat_beta_controls_wing_weight() {
        // Lower beta → heavier wings at fixed FWHM.
        let size = 121;
        let c = size / 2;
        let wing = |beta: f32| {
            render_one(PsfModel::Moffat { fwhm: 4.0, beta }, size, 100.0, 1.0)[c * size + (c + 10)]
        };
        let heavy = wing(2.0);
        let light = wing(6.0);
        assert!(
            heavy > light * 2.0,
            "lower beta should have heavier wings: β2 {heavy:.4} vs β6 {light:.4}"
        );
    }

    #[test]
    fn eccentricity_controls_elongation() {
        // Larger eccentricity → more elongated profile (higher horiz/vert ratio at angle 0).
        let size = 81;
        let c = size / 2;
        let ratio = |e: f32| {
            let p = render_one(
                PsfModel::Elliptical {
                    fwhm: 4.0,
                    eccentricity: e,
                    angle: 0.0,
                },
                size,
                100.0,
                1.0,
            );
            p[c * size + (c + 5)] / p[(c + 5) * size + c]
        };
        let low = ratio(0.3);
        let high = ratio(0.7);
        assert!(
            high > low && low > 1.0,
            "elongation must grow with eccentricity: e0.3 {low:.2}, e0.7 {high:.2}"
        );
    }
}
