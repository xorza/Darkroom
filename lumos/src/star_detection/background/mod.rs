//! Background estimation for star detection.
//!
//! Estimates the sky background using a tiled approach with sigma-clipped
//! statistics, then bilinearly interpolates to create a smooth background map.
//!
//! Uses SIMD acceleration when available for statistics computation.

#[cfg(test)]
mod bench;
mod estimate;
mod simd;
#[cfg(test)]
mod tests;
mod tile_grid;

pub use estimate::BackgroundEstimate;

use crate::common::{BitBuffer2, Buffer2};
use rayon::prelude::*;

use super::buffer_pool::BufferPool;
use super::config::Config;
use tile_grid::TileGrid;

/// Estimate background and noise for the image.
///
/// Performs tiled sigma-clipped statistics with bilinear interpolation.
/// All buffer management is contained within this function.
pub(crate) fn estimate_background(
    pixels: &Buffer2<f32>,
    config: &Config,
    pool: &mut BufferPool,
) -> BackgroundEstimate {
    let width = pixels.width();
    let height = pixels.height();

    assert!(
        width >= config.tile_size && height >= config.tile_size,
        "Image must be at least tile_size x tile_size"
    );

    // Acquire buffers from pool
    let mut background = pool.acquire_f32();
    let mut noise = pool.acquire_f32();

    // Create tile grid and compute statistics
    let mut tile_grid = TileGrid::new_uninit(width, height, config.tile_size);
    tile_grid.compute(pixels, None, config.sigma_clip_iterations);

    // Interpolate from tile grid to per-pixel values
    interpolate_from_grid(&tile_grid, &mut background, &mut noise);

    BackgroundEstimate { background, noise }
}

/// Refine background estimate using iterative object masking.
///
/// Call this after initial estimation when using `BackgroundRefinement::Iterative`.
pub(crate) fn refine_background(
    pixels: &Buffer2<f32>,
    estimate: &mut BackgroundEstimate,
    config: &Config,
    pool: &mut BufferPool,
) {
    let iterations = config.refinement.iterations();
    if iterations == 0 {
        return;
    }

    let width = pixels.width();
    let height = pixels.height();

    let mut tile_grid = TileGrid::new_uninit(width, height, config.tile_size);
    let mut mask = pool.acquire_bit();
    let mut scratch = pool.acquire_bit();

    for _iter in 0..iterations {
        create_object_mask(
            pixels,
            &estimate.background,
            &estimate.noise,
            config.sigma_threshold,
            config.bg_mask_dilation,
            &mut mask,
            &mut scratch,
        );

        tile_grid.compute(pixels, Some(&mask), config.sigma_clip_iterations);

        interpolate_from_grid(&tile_grid, &mut estimate.background, &mut estimate.noise);
    }

    pool.release_bit(scratch);
    pool.release_bit(mask);
}

/// Interpolate background map from tile grid into output buffers.
fn interpolate_from_grid(grid: &TileGrid, background: &mut Buffer2<f32>, noise: &mut Buffer2<f32>) {
    let width = background.width();

    background
        .pixels_mut()
        .par_chunks_mut(width)
        .zip(noise.pixels_mut().par_chunks_mut(width))
        .enumerate()
        .for_each(|(y, (bg_row, noise_row))| {
            interpolate_row(bg_row, noise_row, y, grid);
        });
}

/// Create a mask of pixels that are likely objects (above threshold).
///
/// `output` is used as the mask buffer. `scratch` is used for dilation if needed.
fn create_object_mask(
    pixels: &Buffer2<f32>,
    background: &Buffer2<f32>,
    noise: &Buffer2<f32>,
    detection_sigma: f32,
    dilation_radius: usize,
    output: &mut BitBuffer2,
    scratch: &mut BitBuffer2,
) {
    // Create threshold mask using packed SIMD-optimized implementation
    super::threshold_mask::create_threshold_mask(
        pixels,
        background,
        noise,
        detection_sigma,
        output,
    );

    // Dilate mask to cover object wings
    if dilation_radius > 0 {
        super::mask_dilation::dilate_mask(output, dilation_radius, scratch);
        std::mem::swap(output, scratch);
    }
}

/// Interpolate an entire row using segment-based bilinear interpolation.
///
/// Instead of computing tile indices per-pixel, we process the row in segments
/// where each segment has constant tile corners. This amortizes tile lookups
/// and Y-weight calculations across many pixels.
fn interpolate_row(bg_row: &mut [f32], noise_row: &mut [f32], y: usize, grid: &TileGrid) {
    let fy = y as f32;
    let width = bg_row.len();

    // Compute Y tile indices and weight once for the entire row
    let ty0 = grid.find_lower_tile_y(fy);
    let ty1 = (ty0 + 1).min(grid.tiles_y() - 1);

    let center_y0 = grid.center_y(ty0);
    let center_y1 = grid.center_y(ty1);
    let wy = if ty1 != ty0 {
        ((fy - center_y0) / (center_y1 - center_y0)).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let wy_inv = 1.0 - wy;

    // Process row in segments between tile center X boundaries
    let mut x = 0usize;

    for tx0 in 0..grid.tiles_x() {
        let tx1 = (tx0 + 1).min(grid.tiles_x() - 1);

        // Segment runs from current x to next tile center (or end of row)
        let segment_end = if tx0 + 1 < grid.tiles_x() {
            // Segment ends at next tile center
            (grid.center_x(tx0 + 1).floor() as usize).min(width)
        } else {
            width
        };

        // Skip if segment is empty or we've passed it
        if segment_end <= x {
            continue;
        }

        // Fetch the four corner tiles for this segment
        let t00 = grid.get(tx0, ty0);
        let t10 = grid.get(tx1, ty0);
        let t01 = grid.get(tx0, ty1);
        let t11 = grid.get(tx1, ty1);

        // Precompute Y-blended values for left and right tile columns
        let left_bg = wy_inv * t00.median + wy * t01.median;
        let right_bg = wy_inv * t10.median + wy * t11.median;
        let left_noise = wy_inv * t00.sigma + wy * t01.sigma;
        let right_noise = wy_inv * t10.sigma + wy * t11.sigma;

        let bg_segment = &mut bg_row[x..segment_end];
        let noise_segment = &mut noise_row[x..segment_end];

        let center_x0 = grid.center_x(tx0);
        let center_x1 = grid.center_x(tx1);
        if tx1 != tx0 {
            // Interpolation needed - use SIMD-accelerated version
            let inv_dx = 1.0 / (center_x1 - center_x0);
            let wx_start = (x as f32 - center_x0) * inv_dx;
            let wx_step = inv_dx;

            simd::interpolate_segment_simd(
                bg_segment,
                noise_segment,
                left_bg,
                right_bg,
                left_noise,
                right_noise,
                wx_start,
                wx_step,
            );
        } else {
            // Constant fill (single tile column)
            bg_segment.fill(left_bg);
            noise_segment.fill(left_noise);
        }

        x = segment_end;
        if x >= width {
            break;
        }
    }
}

// =============================================================================
// Test helpers
// =============================================================================

/// Test utility: estimate background with automatic buffer pool management.
#[cfg(test)]
pub(crate) fn estimate_background_test(
    pixels: &Buffer2<f32>,
    config: &Config,
) -> BackgroundEstimate {
    let mut pool = BufferPool::new(pixels.width(), pixels.height());
    let mut estimate = estimate_background(pixels, config, &mut pool);

    if config.refinement.iterations() > 0 {
        refine_background(pixels, &mut estimate, config, &mut pool);
    }

    estimate
}
