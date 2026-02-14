//! Background estimation for star detection.
//!
//! Estimates the sky background using a tiled approach with sigma-clipped
//! statistics, then interpolates using natural bicubic spline to create a
//! C2-continuous background map (matching SExtractor/SEP).
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
/// Performs tiled sigma-clipped statistics with natural bicubic spline interpolation.
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

/// Interpolate an entire row using natural bicubic spline interpolation.
///
/// Two-pass approach matching SExtractor/SEP:
/// 1. Evaluate Y spline at this row for each tile column → node values
/// 2. Solve tridiagonal system in X for second derivatives
/// 3. Evaluate X spline per-pixel using SIMD-accelerated segments
fn interpolate_row(bg_row: &mut [f32], noise_row: &mut [f32], y: usize, grid: &TileGrid) {
    let fy = y as f32;
    let width = bg_row.len();
    let tiles_x = grid.tiles_x();

    // --- Step 1: Evaluate Y spline at each tile column ---

    let ty0 = grid.find_lower_tile_y(fy);
    let ty1 = (ty0 + 1).min(grid.tiles_y() - 1);
    let cy0 = grid.center_y(ty0);
    let cy1 = grid.center_y(ty1);
    let hy = cy1 - cy0;
    let ty = if ty1 != ty0 {
        ((fy - cy0) / hy).clamp(0.0, 1.0)
    } else {
        0.0
    };

    // Evaluate Y cubic spline at each tile column
    // For small tile counts (common), use stack-friendly SmallVec-style approach
    let mut node_bg = vec![0.0f32; tiles_x];
    let mut node_noise = vec![0.0f32; tiles_x];

    for tx in 0..tiles_x {
        let f0_bg = grid.get(tx, ty0).median;
        let f1_bg = grid.get(tx, ty1).median;
        let d0_bg = grid.d2y_median(tx, ty0);
        let d1_bg = grid.d2y_median(tx, ty1);
        node_bg[tx] = tile_grid::cubic_spline_eval(f0_bg, f1_bg, d0_bg, d1_bg, hy, ty);

        let f0_n = grid.get(tx, ty0).sigma;
        let f1_n = grid.get(tx, ty1).sigma;
        let d0_n = grid.d2y_sigma(tx, ty0);
        let d1_n = grid.d2y_sigma(tx, ty1);
        node_noise[tx] = tile_grid::cubic_spline_eval(f0_n, f1_n, d0_n, d1_n, hy, ty);
    }

    // --- Step 2: Solve tridiagonal system in X for second derivatives ---

    let centers_x: Vec<f32> = (0..tiles_x).map(|tx| grid.center_x(tx)).collect();
    let mut d2x_bg = vec![0.0f32; tiles_x];
    let mut d2x_noise = vec![0.0f32; tiles_x];

    tile_grid::solve_natural_spline_d2(&node_bg, &centers_x, &mut d2x_bg);
    tile_grid::solve_natural_spline_d2(&node_noise, &centers_x, &mut d2x_noise);

    // --- Step 3: Evaluate X spline per segment ---

    let mut x = 0usize;

    for tx0 in 0..tiles_x {
        let tx1 = (tx0 + 1).min(tiles_x - 1);

        let segment_end = if tx0 + 1 < tiles_x {
            (grid.center_x(tx0 + 1).floor() as usize).min(width)
        } else {
            width
        };

        if segment_end <= x {
            continue;
        }

        let bg_segment = &mut bg_row[x..segment_end];
        let noise_segment = &mut noise_row[x..segment_end];

        let cx0 = centers_x[tx0];
        let cx1 = centers_x[tx1];

        if tx1 != tx0 {
            let hx = cx1 - cx0;
            let hx2_6 = hx * hx / 6.0;
            let inv_hx = 1.0 / hx;
            let tx_start = (x as f32 - cx0) * inv_hx;
            let tx_step = inv_hx;

            // Precompute spline coefficients: a = h²/6 * d2[left], b = h²/6 * d2[right]
            let bg_f0 = node_bg[tx0];
            let bg_f1 = node_bg[tx1];
            let bg_a = hx2_6 * d2x_bg[tx0];
            let bg_b = hx2_6 * d2x_bg[tx1];

            let noise_f0 = node_noise[tx0];
            let noise_f1 = node_noise[tx1];
            let noise_a = hx2_6 * d2x_noise[tx0];
            let noise_b = hx2_6 * d2x_noise[tx1];

            simd::interpolate_segment_cubic_simd(
                bg_segment,
                noise_segment,
                bg_f0,
                bg_f1,
                bg_a,
                bg_b,
                noise_f0,
                noise_f1,
                noise_a,
                noise_b,
                tx_start,
                tx_step,
            );
        } else {
            // Single tile column — constant fill
            bg_segment.fill(node_bg[tx0]);
            noise_segment.fill(node_noise[tx0]);
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
