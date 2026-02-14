//! Test utilities for the detect stage.

use crate::common::Buffer2;
use crate::star_detection::background::BackgroundEstimate;
use crate::star_detection::buffer_pool::BufferPool;
use crate::star_detection::config::Config;
use crate::star_detection::deblend::Region;

use super::detect::detect;

/// Test utility: detect stars with automatic buffer pool management.
///
/// Creates a temporary buffer pool internally. For benchmarks, use
/// `detect` directly with a pre-allocated pool.
pub fn detect_stars_test(
    pixels: &Buffer2<f32>,
    background: &BackgroundEstimate,
    config: &Config,
) -> Vec<Region> {
    let mut pool = BufferPool::new(pixels.width(), pixels.height());
    detect(pixels, background, None, config, &mut pool).regions
}
