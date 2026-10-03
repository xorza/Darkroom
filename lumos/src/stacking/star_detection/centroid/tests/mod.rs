//! Tests for centroid computation.

use crate::testing::prelude::*;
use std::f32::consts::FRAC_PI_4;

use crate::math::fwhm::{alpha_beta_to_fwhm, fwhm_to_sigma, sigma_to_fwhm};
use crate::math::urect::URect;
use crate::stacking::star_detection::background::background_estimate::BackgroundEstimate;
use crate::stacking::star_detection::centroid::*;
use crate::stacking::star_detection::config::Config;
use crate::stacking::star_detection::config::background_config::BackgroundConfig;
use crate::stacking::star_detection::config::detection_config::DetectionConfig;
use crate::stacking::star_detection::config::measurement_config::MeasurementConfig;
use crate::stacking::star_detection::deblend::region::Region;
use crate::stacking::star_detection::detector::stages::detect::internals::detect_stars_test;
use crate::testing::synthetic::background_map;
use crate::testing::synthetic::patterns;

/// The FWHM most fixtures here are built at: σ = 2.5 → 2.3548 · 2.5 = 5.887.
const TEST_EXPECTED_FWHM: f32 = 5.9;

/// The stamp `measure_star` would use at [`TEST_EXPECTED_FWHM`]: ceil(1.75 · 5.9) = 11.
const TEST_STAMP_RADIUS: usize = compute_stamp_radius(TEST_EXPECTED_FWHM);

use crate::testing::synthetic::star_profiles::{StarProfile, SyntheticStar};

/// How close a profile fit to noiseless samples of its own model comes to the truth, in px².
///
/// The samples are f32, each rounded by up to 2⁻²⁴ ≈ 6e-8 of its value; through the fit that
/// rounding moves the covariance by at most 1.6e-7 px² over the fixtures here (measured), so 1e-6
/// holds it with room while staying far below any width error worth reporting.
const EXACT_FIT_PX2: f64 = 1e-6;

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
            peak_value: self.residual[(px, py)],
            area: 50,
        }
    }

    /// `measure_star` on `region` with its stamp sized from `expected_fwhm`, as the measure stage
    /// runs it.
    fn measure(
        &self,
        region: &Region,
        config: &MeasurementConfig,
        expected_fwhm: f32,
    ) -> Option<Star> {
        measure_star(
            &self.residual,
            &self.sky,
            &self.saturation,
            region,
            config,
            expected_fwhm,
            &StampGrid::new(compute_stamp_radius(expected_fwhm)),
        )
    }

    /// `compute_star` at `pos` over a stamp of `radius`, with the global sky and no noise model.
    fn compute(&self, pos: DVec2, radius: usize) -> Option<Star> {
        compute_star(&self.residual, &self.sky, pos, 0.0, radius, None, None)
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
