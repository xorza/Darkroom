//! Algorithm stage tests - tests individual components of the star detection pipeline.

use crate::stacking::star_detection::background::estimate::BackgroundEstimate;
use crate::stacking::star_detection::config::BackgroundConfig;
use crate::stacking::star_detection::deblend::region::Region;
use crate::testing::estimate_background;
use imaginarium::Buffer2;

/// Default tile size for background estimation.
pub(crate) const TILE_SIZE: usize = 64;

mod background_tests;
mod centroid_tests;
mod convolution_tests;
mod cosmic_ray_tests;
mod deblend_tests;
mod detection_tests;

/// Estimate the background of `pixels` with the stage tests' default tile size.
pub(crate) fn background_estimate(pixels: &Buffer2<f32>) -> BackgroundEstimate {
    estimate_background(
        pixels,
        &BackgroundConfig {
            tile_size: TILE_SIZE,
            ..Default::default()
        },
    )
}

/// Count how many of `truths` `(x, y)` have a candidate peak within `radius` px (each truth at
/// most once).
pub(crate) fn matched_truths(candidates: &[Region], truths: &[(f32, f32)], radius: f32) -> usize {
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
