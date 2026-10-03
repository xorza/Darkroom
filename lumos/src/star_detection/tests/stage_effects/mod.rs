//! Detector runs that isolate one stage's contribution.
//!
//! Every test here runs the detector — `detect_stars_test` or `StarDetector::detect` — on a field
//! built to stress one stage, and grades the detections. Cosmic-ray rejection, deblending and
//! thresholding are visible only in what the detector finally reports, so they cannot be tested
//! against a stage's own API.
//!
//! A test that calls a *single* stage's function directly belongs in that module's own `tests/`,
//! beside the unit tests for the same code — see the placement rule in the parent module.

use crate::internals::prelude::*;
use crate::internals::synthetic::background_map;
use crate::star_detection::background::background_estimate::BackgroundEstimate;
use crate::star_detection::config::background_config::BackgroundConfig;
use crate::star_detection::deblend::region::Region;
use crate::star_detection::tests::near;

mod cosmic_ray_tests;
mod deblend_tests;
mod detection_tests;

/// Estimate the background of `pixels` at the default tile size.
fn background_estimate(pixels: &Buffer2<f32>) -> BackgroundEstimate {
    background_map::estimate(pixels, &BackgroundConfig::default())
}

/// The pixel positions of `regions`' peaks.
fn peaks(regions: &[Region]) -> Vec<DVec2> {
    regions
        .iter()
        .map(|region| DVec2::new(region.peak.x as f64, region.peak.y as f64))
        .collect()
}

/// How many of `truths` have a candidate peak within `radius` px.
fn matched_truths(candidates: &[Region], truths: &[(f32, f32)], radius: f32) -> usize {
    let found = peaks(candidates);
    truths
        .iter()
        .filter(|&&(x, y)| {
            near(
                DVec2::new(f64::from(x), f64::from(y)),
                &found,
                f64::from(radius),
            ) > 0
        })
        .count()
}
