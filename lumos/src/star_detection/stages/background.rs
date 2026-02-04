//! Background estimation stage.
//!
//! Estimates per-pixel background and noise from the image, with optional
//! iterative refinement. Returns an [`ImageStats`] data-only struct.

use crate::common::Buffer2;

use super::super::background::BackgroundMap;
use super::super::buffer_pool::BufferPool;
use super::super::config::Config;
use super::super::image_stats::ImageStats;

/// Estimate background and noise for the image.
///
/// Performs tiled sigma-clipped statistics with bilinear interpolation,
/// optionally followed by iterative refinement with object masking.
/// All buffer management is contained within this function.
pub fn estimate_background(
    pixels: &Buffer2<f32>,
    config: &Config,
    pool: &mut BufferPool,
) -> ImageStats {
    let iterations = config.refinement.iterations();

    let mut background = BackgroundMap::from_pool(pool, config);
    background.estimate(pixels);

    if iterations > 0 {
        let mut scratch1 = pool.acquire_bit();
        let mut scratch2 = pool.acquire_bit();

        background.refine(pixels, &mut scratch1, &mut scratch2);

        pool.release_bit(scratch1);
        pool.release_bit(scratch2);
    }

    background.into_image_stats()
}
