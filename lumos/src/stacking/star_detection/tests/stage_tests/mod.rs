//! Algorithm stage tests - tests individual components of the star detection pipeline.

use crate::stacking::star_detection::background::background_estimate::BackgroundEstimate;
use crate::stacking::star_detection::config::background_config::BackgroundConfig;
use crate::stacking::star_detection::deblend::region::Region;
use crate::testing::prelude::*;

/// Default tile size for background estimation.
const TILE_SIZE: usize = 64;

mod background_tests;
mod centroid_tests;
mod convolution_tests;
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
