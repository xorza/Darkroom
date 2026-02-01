//! Background estimation for star detection.
//!
//! Estimates the sky background using a tiled approach with sigma-clipped
//! statistics, then bilinearly interpolates to create a smooth background map.
//!
//! Uses SIMD acceleration when available for statistics computation.

#[cfg(test)]
mod bench;
#[cfg(test)]
mod tests;

mod simd;
mod tile_grid;

use crate::common::{BitBuffer2, Buffer2};
use common::parallel;
use rayon::prelude::*;

use tile_grid::TileGrid;

// Re-export BackgroundConfig from config module
pub use super::config::BackgroundConfig;

// Re-export AdaptiveSigmaConfig for public API
pub use tile_grid::AdaptiveSigmaConfig;

/// Background map with per-pixel background and noise estimates.
#[derive(Debug, Clone)]
pub struct BackgroundMap {
    /// Per-pixel background values.
    pub background: Buffer2<f32>,
    /// Per-pixel noise (sigma) estimates.
    pub noise: Buffer2<f32>,
    /// Per-pixel adaptive detection threshold in sigma units.
    /// Higher in nebulous/high-contrast regions, lower in uniform sky.
    /// Only populated when adaptive thresholding is enabled.
    pub adaptive_sigma: Option<Buffer2<f32>>,
}

impl BackgroundMap {
    /// Estimate background from image pixels using the given configuration.
    ///
    /// Uses a tiled approach with sigma-clipped statistics:
    /// 1. Divide image into tiles of `tile_size` pixels
    /// 2. For each tile, compute sigma-clipped median and standard deviation
    /// 3. Bilinearly interpolate between tile centers to get per-pixel values
    ///
    /// If `config.iterations > 0`, uses iterative refinement (SExtractor-style):
    /// 1. Estimate initial background
    /// 2. Detect pixels above threshold (potential objects)
    /// 3. Create dilated mask of detected regions
    /// 4. Re-estimate background excluding masked pixels
    /// 5. Repeat for specified iterations
    ///
    /// This provides cleaner background maps for crowded fields by excluding
    /// stars and other bright objects from the estimation.
    pub fn new(pixels: &Buffer2<f32>, config: &BackgroundConfig) -> Self {
        config.validate();
        assert!(
            pixels.width() >= config.tile_size && pixels.height() >= config.tile_size,
            "Image must be at least tile_size x tile_size"
        );

        let grid = TileGrid::new_with_options(
            pixels,
            config.tile_size,
            None,
            0,
            config.sigma_clip_iterations,
            None, // No adaptive thresholding during initial background estimation
        );
        let mut background = interpolate_from_grid(pixels, &grid);

        if config.iterations > 0 {
            refine(&mut background, pixels, config);
        }

        background
    }

    /// Get image width.
    #[inline]
    pub fn width(&self) -> usize {
        self.background.width()
    }

    /// Get image height.
    #[inline]
    pub fn height(&self) -> usize {
        self.background.height()
    }

    /// Get background value at a pixel position.
    #[allow(dead_code)] // Public API for external use
    #[inline]
    pub fn get_background(&self, x: usize, y: usize) -> f32 {
        self.background[(x, y)]
    }

    /// Get noise estimate at a pixel position.
    #[allow(dead_code)] // Public API for external use
    #[inline]
    pub fn get_noise(&self, x: usize, y: usize) -> f32 {
        self.noise[(x, y)]
    }

    /// Estimate background with adaptive sigma thresholds.
    ///
    /// This is similar to `new()` but also computes per-pixel adaptive
    /// detection thresholds based on local contrast. Use this for images
    /// with variable nebulosity, gradients, or other complex backgrounds.
    ///
    /// The adaptive sigma is higher in high-contrast regions (nebulae, gradients)
    /// and lower in uniform sky regions, reducing false positives while
    /// maintaining sensitivity in clean areas.
    pub fn new_with_adaptive_sigma(
        pixels: &Buffer2<f32>,
        config: &BackgroundConfig,
        adaptive_config: AdaptiveSigmaConfig,
    ) -> Self {
        config.validate();
        assert!(
            pixels.width() >= config.tile_size && pixels.height() >= config.tile_size,
            "Image must be at least tile_size x tile_size"
        );

        let grid = TileGrid::new_with_options(
            pixels,
            config.tile_size,
            None,
            0,
            config.sigma_clip_iterations,
            Some(adaptive_config),
        );
        let mut background = interpolate_from_grid_with_adaptive(pixels, &grid);

        if config.iterations > 0 {
            refine(&mut background, pixels, config);
        }

        background
    }
}

/// Interpolate from grid including adaptive sigma channel.
fn interpolate_from_grid_with_adaptive(pixels: &Buffer2<f32>, grid: &TileGrid) -> BackgroundMap {
    let width = pixels.width();
    let height = pixels.height();

    let mut bg_data = Buffer2::new_filled(width, height, 0.0);
    let mut noise_data = Buffer2::new_filled(width, height, 0.0);
    let mut adaptive_data = Buffer2::new_filled(width, height, 0.0);

    // Process in parallel chunks
    let bg_ptr = bg_data.pixels_mut().as_mut_ptr() as usize;
    let noise_ptr = noise_data.pixels_mut().as_mut_ptr() as usize;
    let adaptive_ptr = adaptive_data.pixels_mut().as_mut_ptr() as usize;

    parallel::par_iter_auto(height).for_each(|(_, start_row, end_row)| {
        for y in start_row..end_row {
            let row_offset = y * width;

            // SAFETY: Each row is processed by only one thread
            let bg_row = unsafe {
                std::slice::from_raw_parts_mut((bg_ptr as *mut f32).add(row_offset), width)
            };
            let noise_row = unsafe {
                std::slice::from_raw_parts_mut((noise_ptr as *mut f32).add(row_offset), width)
            };
            let adaptive_row = unsafe {
                std::slice::from_raw_parts_mut((adaptive_ptr as *mut f32).add(row_offset), width)
            };

            interpolate_row(bg_row, noise_row, Some(adaptive_row), y, grid);
        }
    });

    BackgroundMap {
        background: bg_data,
        noise: noise_data,
        adaptive_sigma: Some(adaptive_data),
    }
}

fn interpolate_from_grid(pixels: &Buffer2<f32>, grid: &TileGrid) -> BackgroundMap {
    let width = pixels.width();
    let height = pixels.height();

    let mut background = BackgroundMap {
        background: Buffer2::new_filled(width, height, 0.0),
        noise: Buffer2::new_filled(width, height, 0.0),
        adaptive_sigma: None,
    };

    parallel::par_chunks_auto_aligned_zip2(
        background.background.pixels_mut(),
        background.noise.pixels_mut(),
        width,
    )
    .for_each(|(chunk_start_row, (bg_chunk, noise_chunk))| {
        let rows_in_chunk = bg_chunk.len() / width;

        for local_y in 0..rows_in_chunk {
            let y = chunk_start_row + local_y;
            let row_offset = local_y * width;
            let bg_row = &mut bg_chunk[row_offset..row_offset + width];
            let noise_row = &mut noise_chunk[row_offset..row_offset + width];

            interpolate_row(bg_row, noise_row, None, y, grid);
        }
    });

    background
}

fn refine(background: &mut BackgroundMap, pixels: &Buffer2<f32>, config: &BackgroundConfig) {
    let width = pixels.width();
    let height = pixels.height();

    let mut mask = BitBuffer2::new_filled(width, height, false);
    let mut scratch = BitBuffer2::new_filled(width, height, false);

    for _iter in 0..config.iterations {
        create_object_mask(
            pixels,
            background,
            config.sigma_threshold,
            config.mask_dilation,
            &mut mask,
            &mut scratch,
        );

        estimate_background_masked(
            pixels,
            config.tile_size,
            &mask,
            config.min_unmasked_fraction,
            config.sigma_clip_iterations,
            background,
        );
    }
}

/// Create a mask of pixels that are likely objects (above threshold).
///
/// `output` is used as the mask buffer. `scratch` is used for dilation if needed.
fn create_object_mask(
    pixels: &Buffer2<f32>,
    background: &BackgroundMap,
    detection_sigma: f32,
    dilation_radius: usize,
    output: &mut BitBuffer2,
    scratch: &mut BitBuffer2,
) {
    // Create threshold mask using packed SIMD-optimized implementation
    super::common::threshold_mask::create_threshold_mask(
        pixels,
        &background.background,
        &background.noise,
        detection_sigma,
        output,
    );

    // Dilate mask to cover object wings
    if dilation_radius > 0 {
        super::common::dilate_mask(output, dilation_radius, scratch);
        std::mem::swap(output, scratch);
    }
}

/// Estimate background with masked pixels excluded.
fn estimate_background_masked(
    pixels: &Buffer2<f32>,
    tile_size: usize,
    mask: &BitBuffer2,
    min_unmasked_fraction: f32,
    sigma_clip_iterations: usize,
    output: &mut BackgroundMap,
) {
    let width = pixels.width();
    let height = pixels.height();

    assert!(
        (16..=256).contains(&tile_size),
        "Tile size must be between 16 and 256"
    );
    assert!(
        width >= tile_size && height >= tile_size,
        "Image must be at least tile_size x tile_size"
    );

    let max_tile_pixels = tile_size * tile_size;
    let min_pixels = (max_tile_pixels as f32 * min_unmasked_fraction) as usize;

    let grid = TileGrid::new_with_options(
        pixels,
        tile_size,
        Some(mask),
        min_pixels,
        sigma_clip_iterations,
        None, // No adaptive thresholding during refinement
    );

    output.background = Buffer2::new_filled(width, height, 0.0);
    output.noise = Buffer2::new_filled(width, height, 0.0);
    output.adaptive_sigma = None;

    parallel::par_chunks_auto_aligned_zip2(
        output.background.pixels_mut(),
        output.noise.pixels_mut(),
        width,
    )
    .for_each(|(chunk_start_row, (bg_chunk, noise_chunk))| {
        let rows_in_chunk = bg_chunk.len() / width;

        for local_y in 0..rows_in_chunk {
            let y = chunk_start_row + local_y;
            let row_offset = local_y * width;
            let bg_row = &mut bg_chunk[row_offset..row_offset + width];
            let noise_row = &mut noise_chunk[row_offset..row_offset + width];

            interpolate_row(bg_row, noise_row, None, y, &grid);
        }
    });
}

/// Interpolate an entire row using segment-based bilinear interpolation.
///
/// Instead of computing tile indices per-pixel, we process the row in segments
/// where each segment has constant tile corners. This amortizes tile lookups
/// and Y-weight calculations across many pixels.
///
/// If `adaptive_row` is Some, also interpolates the adaptive_sigma channel.
fn interpolate_row(
    bg_row: &mut [f32],
    noise_row: &mut [f32],
    adaptive_row: Option<&mut [f32]>,
    y: usize,
    grid: &TileGrid,
) {
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

        // Interpolate adaptive sigma if requested
        if let Some(ref adaptive_row) = adaptive_row {
            let left_adaptive = wy_inv * t00.adaptive_sigma + wy * t01.adaptive_sigma;
            let right_adaptive = wy_inv * t10.adaptive_sigma + wy * t11.adaptive_sigma;

            // Get mutable reference to adaptive segment
            // SAFETY: We have exclusive access via the Option<&mut [f32]>
            let adaptive_segment = unsafe {
                std::slice::from_raw_parts_mut(
                    adaptive_row.as_ptr().add(x) as *mut f32,
                    segment_end - x,
                )
            };

            if tx1 != tx0 {
                let inv_dx = 1.0 / (center_x1 - center_x0);
                let wx_start = (x as f32 - center_x0) * inv_dx;
                let wx_step = inv_dx;

                // Simple scalar interpolation for adaptive sigma (less performance critical)
                let delta = right_adaptive - left_adaptive;
                for (i, val) in adaptive_segment.iter_mut().enumerate() {
                    let wx = (wx_start + i as f32 * wx_step).clamp(0.0, 1.0);
                    *val = left_adaptive + wx * delta;
                }
            } else {
                adaptive_segment.fill(left_adaptive);
            }
        }

        x = segment_end;
        if x >= width {
            break;
        }
    }
}
