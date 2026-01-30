//! Background estimation for star detection.
//!
//! Estimates the sky background using a tiled approach with sigma-clipped
//! statistics, then bilinearly interpolates to create a smooth background map.
//!
//! Uses SIMD acceleration when available for statistics computation.

#[cfg(test)]
mod tests;

mod simd;
mod tile_grid;

use crate::common::Buffer2;
use crate::common::parallel::ParZipMut;
use rayon::prelude::*;

use tile_grid::TileGrid;

/// Background map with per-pixel background and noise estimates.
#[derive(Debug, Clone)]
pub struct BackgroundMap {
    /// Per-pixel background values.
    pub background: Buffer2<f32>,
    /// Per-pixel noise (sigma) estimates.
    pub noise: Buffer2<f32>,
}

impl BackgroundMap {
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

    /// Get background-subtracted value at a pixel position.
    #[allow(dead_code)] // Public API for external use
    #[inline]
    pub fn subtract(&self, pixels: &[f32], x: usize, y: usize) -> f32 {
        let idx = y * self.width() + x;
        pixels[idx] - self.background[(x, y)]
    }
}

/// Configuration for iterative background refinement.
#[derive(Debug, Clone)]
pub struct BackgroundConfig {
    /// Detection threshold in sigma above background for masking objects.
    /// Higher values = more conservative masking (only mask very bright objects).
    /// Typical value: 3.0-5.0
    pub detection_sigma: f32,
    /// Number of refinement iterations. Usually 1-2 is sufficient.
    pub iterations: usize,
    /// Dilation radius for object masks in pixels.
    /// Expands masked regions to ensure object wings are excluded.
    /// Typical value: 2-5 pixels.
    pub mask_dilation: usize,
    /// Minimum fraction of pixels that must remain unmasked per tile.
    /// If too many pixels are masked, use original (unrefined) estimate.
    /// Typical value: 0.3-0.5
    pub min_unmasked_fraction: f32,

    pub tile_size: usize,
}

impl Default for BackgroundConfig {
    fn default() -> Self {
        Self {
            detection_sigma: 4.0,
            iterations: 0,
            mask_dilation: 3,
            min_unmasked_fraction: 0.3,
            tile_size: 64,
        }
    }
}

impl BackgroundConfig {
    /// Estimate background using this configuration.
    ///
    /// Uses a tiled approach with sigma-clipped statistics:
    /// 1. Divide image into tiles of `tile_size` pixels
    /// 2. For each tile, compute sigma-clipped median and standard deviation
    /// 3. Bilinearly interpolate between tile centers to get per-pixel values
    ///
    /// If `iterations > 0`, uses iterative refinement (SExtractor-style):
    /// 1. Estimate initial background
    /// 2. Detect pixels above threshold (potential objects)
    /// 3. Create dilated mask of detected regions
    /// 4. Re-estimate background excluding masked pixels
    /// 5. Repeat for specified iterations
    ///
    /// This provides cleaner background maps for crowded fields by excluding
    /// stars and other bright objects from the estimation.
    pub fn estimate(&self, pixels: &Buffer2<f32>) -> BackgroundMap {
        self.validate();
        self.validate_image_size(pixels);

        let grid = TileGrid::new(pixels, self.tile_size);

        let mut background = self.interpolate_background(pixels, &grid);

        self.refine_background(pixels, &mut background);

        background
    }

    fn validate_image_size(&self, pixels: &Buffer2<f32>) {
        assert!(
            pixels.width() >= self.tile_size && pixels.height() >= self.tile_size,
            "Image must be at least tile_size x tile_size"
        );
    }

    fn interpolate_background(&self, pixels: &Buffer2<f32>, grid: &TileGrid) -> BackgroundMap {
        let width = pixels.width();
        let height = pixels.height();

        let mut background = BackgroundMap {
            background: Buffer2::new_filled(width, height, 0.0),
            noise: Buffer2::new_filled(width, height, 0.0),
        };

        background
            .background
            .pixels_mut()
            .par_zip(background.noise.pixels_mut())
            .par_rows_mut_auto(width)
            .for_each(|(chunk_start_row, (bg_chunk, noise_chunk))| {
                let rows_in_chunk = bg_chunk.len() / width;

                for local_y in 0..rows_in_chunk {
                    let y = chunk_start_row + local_y;
                    let row_offset = local_y * width;
                    let bg_row = &mut bg_chunk[row_offset..row_offset + width];
                    let noise_row = &mut noise_chunk[row_offset..row_offset + width];

                    interpolate_row(bg_row, noise_row, y, grid);
                }
            });

        background
    }

    fn refine_background(&self, pixels: &Buffer2<f32>, background: &mut BackgroundMap) {
        if self.iterations == 0 {
            return;
        }

        let width = pixels.width();
        let height = pixels.height();

        let mut mask = Buffer2::new_filled(width, height, false);
        let mut scratch = Buffer2::new_filled(width, height, false);

        for _iter in 0..self.iterations {
            create_object_mask(
                pixels,
                background,
                self.detection_sigma,
                self.mask_dilation,
                &mut mask,
                &mut scratch,
            );

            estimate_background_masked(
                pixels,
                self.tile_size,
                &mask,
                self.min_unmasked_fraction,
                background,
            );
        }
    }

    /// Validate the configuration and panic if invalid.
    ///
    /// # Panics
    /// Panics with a descriptive message if any parameter is out of valid range.
    pub fn validate(&self) {
        assert!(
            self.detection_sigma > 0.0,
            "detection_sigma must be positive, got {}",
            self.detection_sigma
        );
        assert!(
            self.iterations <= 10,
            "iterations must be <= 10, got {}",
            self.iterations
        );
        assert!(
            self.mask_dilation <= 50,
            "mask_dilation must be <= 50, got {}",
            self.mask_dilation
        );
        assert!(
            (0.0..=1.0).contains(&self.min_unmasked_fraction),
            "min_unmasked_fraction must be in [0, 1], got {}",
            self.min_unmasked_fraction
        );
        assert!(
            (16..=256).contains(&self.tile_size),
            "Tile size must be between 16 and 256"
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
    output: &mut Buffer2<bool>,
    scratch: &mut Buffer2<bool>,
) {
    // Create threshold mask using SIMD-optimized implementation
    super::common::threshold_mask::create_threshold_mask(
        pixels,
        background,
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
    mask: &Buffer2<bool>,
    min_unmasked_fraction: f32,
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

    let grid = TileGrid::new_with_mask(pixels, tile_size, Some(mask), min_pixels);

    output.background = Buffer2::new_filled(width, height, 0.0);
    output.noise = Buffer2::new_filled(width, height, 0.0);

    output
        .background
        .pixels_mut()
        .par_zip(output.noise.pixels_mut())
        .par_rows_mut_auto(width)
        .for_each(|(chunk_start_row, (bg_chunk, noise_chunk))| {
            let rows_in_chunk = bg_chunk.len() / width;

            for local_y in 0..rows_in_chunk {
                let y = chunk_start_row + local_y;
                let row_offset = local_y * width;
                let bg_row = &mut bg_chunk[row_offset..row_offset + width];
                let noise_row = &mut noise_chunk[row_offset..row_offset + width];

                interpolate_row(bg_row, noise_row, y, &grid);
            }
        });
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
