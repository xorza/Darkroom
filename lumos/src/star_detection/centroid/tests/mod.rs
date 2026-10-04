//! Tests for centroid computation.

#![expect(
    clippy::cast_sign_loss,
    reason = "test fixtures are small images, with non-negative coordinates and offsets of a few dozen pixels"
)]

use crate::internals::prelude::*;
use std::f32::consts::FRAC_PI_4;

use crate::internals::synthetic::background_map;
use crate::internals::synthetic::patterns;
use crate::math::fwhm::{FWHM_PER_SIGMA, alpha_beta_to_fwhm, fwhm_to_sigma, sigma_to_fwhm};
use crate::math::urect::URect;
use crate::star_detection::background::background_estimate::BackgroundEstimate;
use crate::star_detection::centroid::measure_grid::MeasureGrid;
use crate::star_detection::centroid::stamp::StampGrid;
use crate::star_detection::centroid::star_noise::StarNoise;
use crate::star_detection::centroid::windowed_centroid::{WindowedCentroid, WindowedInputs};
use crate::star_detection::centroid::*;
use crate::star_detection::config::Config;
use crate::star_detection::config::background_config::BackgroundConfig;
use crate::star_detection::config::detection_config::DetectionConfig;
use crate::star_detection::config::measurement_config::MeasurementConfig;
use crate::star_detection::deblend::region::Region;
use crate::star_detection::detector::stages::detect::internals::detect_stars_test;

/// The FWHM most fixtures here are built at: σ = 2.5 → 2.3548 · 2.5 = 5.887.
const TEST_EXPECTED_FWHM: f32 = 5.9;

/// The stamp `measure_star` would use at [`TEST_EXPECTED_FWHM`]: ceil(1.75 · 5.9) = 11.
const TEST_STAMP_RADIUS: usize = MeasureGrid::stamp_radius(TEST_EXPECTED_FWHM);

use crate::internals::synthetic::star_profiles::{StarProfile, SyntheticStar};

/// How close a profile fit to noiseless samples of its own model comes to the truth, in px².
///
/// The samples are f32, each rounded by up to 2⁻²⁴ ≈ 6e-8 of its value; through the fit that
/// rounding moves the covariance by at most 1.6e-7 px² over the fixtures here (measured), so 1e-6
/// holds it with room while staying far below any width error worth reporting.
const EXACT_FIT_PX2: f64 = 1e-6;

/// The background `measure_star` measures against where no annulus measured the sky: no offset,
/// the map's noise at the nearest pixel held to the floor, and no gain.
fn global_background(sky: &SkyNoise, pos: DVec2) -> StarBackground {
    let pixel = (pos.x.round() as usize, pos.y.round() as usize);
    StarBackground {
        offset: 0.0,
        noise: StarNoise {
            background_sigma: f64::from(sky.noise[pixel].max(sky.floor)),
            electrons_per_unit: None,
        },
        sky_samples: None,
    }
}

/// What the moment metrics read for an axis-aligned Gaussian of `(σx, σy)` over pixels: its own
/// variances plus the window's first-order response to the box's kurtosis `−1/120` on each axis,
/// `1/(240·(σ_w² + s²))`, with `s² = σ² + 1/12` the axis' sampled variance and `σ_w²` the matched
/// window, their mean. The second order is under 2.3e-5 px² from σ 1.5.
#[derive(Debug, Clone, Copy)]
struct MomentReading {
    fwhm: f32,
    eccentricity: f32,
}

impl MomentReading {
    fn of(sigma_x: f32, sigma_y: f32) -> Self {
        let sampled = |sigma: f32| f64::from(sigma).powi(2) + 1.0 / 12.0;
        let window = f64::midpoint(sampled(sigma_x), sampled(sigma_y));
        let read =
            |sigma: f32| f64::from(sigma).powi(2) + 1.0 / (240.0 * (window + sampled(sigma)));
        let (read_x, read_y) = (read(sigma_x), read(sigma_y));
        Self {
            fwhm: (FWHM_PER_SIGMA * (read_x * read_y).powf(0.25)) as f32,
            eccentricity: (1.0 - read_x.min(read_y) / read_x.max(read_y)).sqrt() as f32,
        }
    }
}

/// A saturation mask with no pixel set, the size of `pixels`.
fn unsaturated(pixels: &Buffer2<f32>) -> BitBuffer2 {
    BitBuffer2::new_filled(Size2us::new(pixels.width(), pixels.height()), false)
}

/// A frame as the measurement stage sees it: the residual, the sky noise, and no saturation.
#[derive(Debug)]
struct Measured {
    residual: Buffer2<f32>,
    sky: SkyNoise,
    saturation: BitBuffer2,
}

impl Measured {
    /// `pixels` less `background`.
    fn of(pixels: &Buffer2<f32>, background: &BackgroundEstimate) -> Self {
        Self {
            residual: background.residual_of(pixels),
            sky: background.sky_noise(),
            saturation: unsaturated(pixels),
        }
    }

    /// `pixels` less a flat sky of `level`, with noise `noise`.
    fn flat(pixels: &Buffer2<f32>, level: f32, noise: f32) -> Self {
        let size = Size2us::new(pixels.width(), pixels.height());
        Self::of(pixels, &background_map::uniform(size, level, noise))
    }

    /// The region detection hands `measure_star` for a lone star near `pos`: its 11×11
    /// neighbourhood, peaked at the nearest pixel.
    fn region_at(&self, pos: DVec2) -> Region {
        let (px, py) = (pos.x.round() as usize, pos.y.round() as usize);
        let (width, height) = (self.residual.width(), self.residual.height());
        Region {
            bbox: URect::new(
                Vec2us::new(px.saturating_sub(5), py.saturating_sub(5)),
                Vec2us::new((px + 6).min(width), (py + 6).min(height)),
            ),
            peak: Vec2us::new(px, py),
            area: 50,
        }
    }

    /// `measure_star` on `region` with the grid of `expected_fwhm`, as the measure stage runs it.
    fn measure(
        &self,
        region: &Region,
        config: &MeasurementConfig,
        expected_fwhm: f32,
    ) -> Option<Star> {
        measure_star(
            MeasurePlanes {
                residual: &self.residual,
                sky: &self.sky,
                saturation: &self.saturation,
                no_data: None,
            },
            region,
            config,
            &MeasureGrid::new(expected_fwhm),
        )
    }

    /// The windowed centroid from `start` with the grid of `expected_fwhm`, the global sky, the
    /// test noise of 0.01 and no noise model.
    fn windowed(&self, start: DVec2, expected_fwhm: f32) -> Option<WindowedCentroid> {
        WindowedCentroid::measure(
            &self.residual,
            start,
            &MeasureGrid::new(expected_fwhm),
            WindowedInputs {
                offset: 0.0,
                noise: StarNoise {
                    background_sigma: 0.01,
                    electrons_per_unit: None,
                },
            },
        )
    }

    /// `compute_star` at `pos` over a stamp of `radius`, with the global sky, no gain and the PSF
    /// of [`TEST_EXPECTED_FWHM`].
    fn compute(&self, pos: DVec2, radius: usize) -> Option<Star> {
        compute_star(
            &self.residual,
            pos,
            radius,
            MeasureGrid::new(TEST_EXPECTED_FWHM).window_sigma,
            global_background(&self.sky, pos),
        )
    }
}

mod basic;
mod convergence;
mod fitting;
mod measurement;
pub(crate) mod perturbation;
mod profile_metrics;
mod robustness;
mod stamps;
mod subpixel_recovery;
