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
use rayon::iter::ParallelIterator;

use super::buffer_pool::BufferPool;
use super::config::{BackgroundRefinement, Config};
use super::image_stats::ImageStats;
use tile_grid::TileGrid;

/// Background map with per-pixel background and noise estimates.
#[derive(Debug)]
pub struct BackgroundMap {
    // Stored config fields needed by estimate/refine
    tile_size: usize,
    sigma_clip_iterations: usize,
    refinement: BackgroundRefinement,
    sigma_threshold: f32,
    bg_mask_dilation: usize,
    min_unmasked_fraction: f32,

    /// Tile grid for intermediate statistics (reused across estimates).
    tile_grid: TileGrid,
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
    /// Create an uninitialized BackgroundMap with given dimensions.
    ///
    /// Buffers are allocated but not filled with meaningful values.
    /// Use `estimate` to populate them.
    #[allow(dead_code)] // Used by tests and benchmarks
    pub(crate) fn new_uninit(width: usize, height: usize, config: &Config) -> Self {
        assert!(
            (16..=256).contains(&config.tile_size),
            "tile_size must be between 16 and 256, got {}",
            config.tile_size
        );
        let has_adaptive = config.refinement.adaptive_sigma().is_some();
        Self {
            tile_size: config.tile_size,
            sigma_clip_iterations: config.sigma_clip_iterations,
            refinement: config.refinement,
            sigma_threshold: config.sigma_threshold,
            bg_mask_dilation: config.bg_mask_dilation,
            min_unmasked_fraction: config.min_unmasked_fraction,
            tile_grid: TileGrid::new_uninit(width, height, config.tile_size),
            background: Buffer2::new_default(width, height),
            noise: Buffer2::new_default(width, height),
            adaptive_sigma: if has_adaptive {
                Some(Buffer2::new_default(width, height))
            } else {
                None
            },
        }
    }

    /// Create a BackgroundMap by acquiring buffers from a pool.
    pub(crate) fn from_pool(pool: &mut BufferPool, config: &Config) -> Self {
        let has_adaptive = config.refinement.adaptive_sigma().is_some();
        let width = pool.width();
        let height = pool.height();
        Self {
            tile_size: config.tile_size,
            sigma_clip_iterations: config.sigma_clip_iterations,
            refinement: config.refinement,
            sigma_threshold: config.sigma_threshold,
            bg_mask_dilation: config.bg_mask_dilation,
            min_unmasked_fraction: config.min_unmasked_fraction,
            tile_grid: TileGrid::new_uninit(width, height, config.tile_size),
            background: pool.acquire_f32(),
            noise: pool.acquire_f32(),
            adaptive_sigma: if has_adaptive {
                Some(pool.acquire_f32())
            } else {
                None
            },
        }
    }

    /// Convert into an [`ImageStats`], consuming the BackgroundMap.
    pub(crate) fn into_image_stats(self) -> ImageStats {
        ImageStats {
            background: self.background,
            noise: self.noise,
            adaptive_sigma: self.adaptive_sigma,
        }
    }

    /// Get image width.
    #[inline]
    #[allow(dead_code)]
    pub(crate) fn width(&self) -> usize {
        self.background.width()
    }

    /// Get image height.
    #[inline]
    #[allow(dead_code)]
    pub(crate) fn height(&self) -> usize {
        self.background.height()
    }

    /// Estimate background into this pre-allocated BackgroundMap.
    ///
    /// This variant reuses the existing buffers, avoiding allocation overhead.
    /// Use with `new_uninit` for buffer pooling scenarios.
    ///
    /// For iterative refinement, call `refine` after this method.
    ///
    /// # Panics
    /// Panics if the buffer dimensions don't match the image dimensions.
    pub(crate) fn estimate(&mut self, pixels: &Buffer2<f32>) {
        debug_assert_eq!(self.background.width(), pixels.width());
        debug_assert_eq!(self.background.height(), pixels.height());
        assert!(
            pixels.width() >= self.tile_size && pixels.height() >= self.tile_size,
            "Image must be at least tile_size x tile_size"
        );

        // Only compute adaptive sigma stats if we have the buffer allocated
        let adaptive_config = self.refinement.adaptive_sigma();

        self.tile_grid
            .compute(pixels, None, 0, self.sigma_clip_iterations, adaptive_config);

        interpolate_from_grid(
            &self.tile_grid,
            &mut self.background,
            &mut self.noise,
            self.adaptive_sigma.as_mut(),
        );
    }

    /// Refine background estimate using iterative object masking.
    ///
    /// Call this after `estimate` when using `BackgroundRefinement::Iterative`.
    /// Requires pre-allocated bit buffers for mask and scratch space.
    pub(crate) fn refine(
        &mut self,
        pixels: &Buffer2<f32>,
        scratch1: &mut BitBuffer2,
        scratch2: &mut BitBuffer2,
    ) {
        debug_assert!(
            self.adaptive_sigma.is_none(),
            "adaptive_sigma should not be allocated when using iterative refinement"
        );

        let iterations = self.refinement.iterations();
        debug_assert!(
            iterations > 0,
            "refine() called but no iterations configured"
        );

        let max_tile_pixels = self.tile_size * self.tile_size;
        let min_pixels = (max_tile_pixels as f32 * self.min_unmasked_fraction) as usize;

        let mask = scratch1;
        for _iter in 0..iterations {
            create_object_mask(
                pixels,
                self,
                self.sigma_threshold,
                self.bg_mask_dilation,
                mask,
                scratch2,
            );

            self.tile_grid.compute(
                pixels,
                Some(mask),
                min_pixels,
                self.sigma_clip_iterations,
                None, // No adaptive thresholding during refinement
            );

            interpolate_from_grid(&self.tile_grid, &mut self.background, &mut self.noise, None);
        }
    }
}

/// Interpolate background map from tile grid into output buffers.
///
/// If `adaptive_sigma` is Some, also interpolates the adaptive_sigma channel.
fn interpolate_from_grid(
    grid: &TileGrid,
    background: &mut Buffer2<f32>,
    noise: &mut Buffer2<f32>,
    adaptive_sigma: Option<&mut Buffer2<f32>>,
) {
    let width = background.width();
    let height = background.height();

    // Process in parallel chunks
    let bg_ptr = background.pixels_mut().as_mut_ptr() as usize;
    let noise_ptr = noise.pixels_mut().as_mut_ptr() as usize;
    let adaptive_ptr = adaptive_sigma.map(|b| b.pixels_mut().as_mut_ptr() as usize);

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
            let adaptive_row = adaptive_ptr.map(|ptr| unsafe {
                std::slice::from_raw_parts_mut((ptr as *mut f32).add(row_offset), width)
            });

            interpolate_row(bg_row, noise_row, adaptive_row, y, grid);
        }
    });
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
    super::threshold_mask::create_threshold_mask(
        pixels,
        &background.background,
        &background.noise,
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
