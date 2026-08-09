//! Tests for centroid computation.

use crate::testing::prelude::*;
use std::f32::consts::FRAC_PI_4;

use crate::math::fwhm::FWHM_TO_SIGMA;
use crate::math::rect::URect;
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

mod basic;
mod convergence;
mod fitting;
mod measurement;
mod profile_metrics;
mod robustness;
mod stamps;
