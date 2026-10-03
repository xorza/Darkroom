//! Tests for centroid computation.

use crate::testing::prelude::*;
use std::f32::consts::FRAC_PI_4;

use crate::math::fwhm::{fwhm_to_sigma, sigma_to_fwhm};
use crate::math::urect::URect;
use crate::stacking::star_detection::background::background_estimate::BackgroundEstimate;
use crate::stacking::star_detection::centroid::moffat_fit::alpha_beta_to_fwhm;
use crate::stacking::star_detection::centroid::*;
use crate::stacking::star_detection::config::Config;
use crate::stacking::star_detection::config::background_config::BackgroundConfig;
use crate::stacking::star_detection::config::detection_config::DetectionConfig;
use crate::stacking::star_detection::config::fwhm_config::FwhmConfig;
use crate::stacking::star_detection::config::measurement_config::MeasurementConfig;
use crate::stacking::star_detection::deblend::region::Region;
use crate::stacking::star_detection::detector::stages::detect::internals::detect_stars_test;
use crate::testing::synthetic::patterns;

/// Default stamp radius for tests (matching expected FWHM of ~4 pixels).
const TEST_STAMP_RADIUS: usize = 7;

/// Default expected FWHM for tests (sigma=2.5 -> FWHM≈5.9 pixels).
const TEST_EXPECTED_FWHM: f32 = 5.9;

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

mod basic;
mod convergence;
mod fitting;
mod measurement;
pub(crate) mod perturbation;
mod profile_metrics;
mod robustness;
mod stamps;
mod subpixel_recovery;
