//! Tests for centroid computation.

use std::f32::consts::FRAC_PI_4;

use glam::Vec2;

use crate::math::FWHM_TO_SIGMA;
use crate::math::rect::URect;
use crate::stacking::star_detection::background::estimate::BackgroundEstimate;
use crate::stacking::star_detection::centroid::compute_roundness;
use crate::stacking::star_detection::centroid::moffat_fit::alpha_beta_to_fwhm;
use crate::stacking::star_detection::centroid::test_utils::add_noise;
use crate::stacking::star_detection::centroid::test_utils::make_elliptical_star;
use crate::stacking::star_detection::centroid::*;
use crate::stacking::star_detection::config::{
    BackgroundConfig, Config, DetectionConfig, FwhmConfig, MeasurementConfig,
};
use crate::stacking::star_detection::deblend::region::Region;
use crate::stacking::star_detection::detector::stages::detect_test_utils::detect_stars_test;
use crate::testing::estimate_background;
use crate::testing::synthetic::background_map;
use common::Vec2us;
use imaginarium::Buffer2;

/// Default stamp radius for tests (matching expected FWHM of ~4 pixels).
const TEST_STAMP_RADIUS: usize = 7;

/// Default expected FWHM for tests (sigma=2.5 -> FWHM≈5.9 pixels).
const TEST_EXPECTED_FWHM: f32 = 5.9;

use crate::stacking::star_detection::centroid::test_utils::make_gaussian_star;

fn make_uniform_background(
    width: usize,
    height: usize,
    bg_value: f32,
    noise: f32,
) -> BackgroundEstimate {
    let mut bg_buf = Buffer2::new_default(width, height);
    let mut noise_buf = Buffer2::new_default(width, height);
    bg_buf.fill(bg_value);
    noise_buf.fill(noise);
    BackgroundEstimate {
        background: bg_buf,
        noise: noise_buf,
    }
}

mod basic;
mod convergence;
mod fitting;
mod measurement;
mod profile_metrics;
mod robustness;
mod stamps;
