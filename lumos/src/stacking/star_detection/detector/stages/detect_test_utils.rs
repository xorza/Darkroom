//! Test utilities for the detect stage.

use crate::stacking::star_detection::background::estimate::BackgroundEstimate;
use crate::stacking::star_detection::buffer_pool::BufferPool;
use crate::stacking::star_detection::config::DetectionConfig;
use crate::stacking::star_detection::deblend::region::Region;
use imaginarium::Buffer2;

use crate::stacking::star_detection::detector::stages::detect::detect;

/// Test utility: detect stars with automatic buffer pool management.
///
/// Creates a temporary buffer pool internally. For benchmarks, use
/// `detect` directly with a pre-allocated pool.
pub(crate) fn detect_stars_test(
    pixels: &Buffer2<f32>,
    background: &BackgroundEstimate,
    config: &DetectionConfig,
) -> Vec<Region> {
    let mut pool = BufferPool::new(pixels.width(), pixels.height());
    detect(pixels, background, None, config, &mut pool).regions
}
