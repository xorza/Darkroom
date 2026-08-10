//! Detector runs that isolate one stage's contribution.
//!
//! Every test here runs the detector — `detect_stars_test` or `StarDetector::detect` — on a field
//! built to stress one stage, and grades the detections. Cosmic-ray rejection, deblending and
//! thresholding are visible only in what the detector finally reports, so they cannot be tested
//! against a stage's own API.
//!
//! A test that calls a *single* stage's function directly belongs in that module's own `tests/`,
//! beside the unit tests for the same code — see the placement rule in the parent module.

use crate::stacking::star_detection::background::background_estimate::BackgroundEstimate;
use crate::stacking::star_detection::config::background_config::BackgroundConfig;
use crate::stacking::star_detection::deblend::region::Region;
use crate::testing::prelude::*;

/// Default tile size for background estimation.
const TILE_SIZE: usize = 64;

mod cosmic_ray_tests;
mod deblend_tests;
mod detection_tests;

/// Estimate the background of `pixels` with the stage tests' default tile size.
fn background_estimate(pixels: &Buffer2<f32>) -> BackgroundEstimate {
    background_map::estimate(
        pixels,
        &BackgroundConfig {
            tile_size: TILE_SIZE,
            ..Default::default()
        },
    )
}

/// Count how many of `truths` `(x, y)` have a candidate peak within `radius` px (each truth at
/// most once).
fn matched_truths(candidates: &[Region], truths: &[(f32, f32)], radius: f32) -> usize {
    truths
        .iter()
        .filter(|&&(tx, ty)| {
            candidates.iter().any(|c| {
                let dx = c.peak.x as f32 - tx;
                let dy = c.peak.y as f32 - ty;
                (dx * dx + dy * dy).sqrt() < radius
            })
        })
        .count()
}
