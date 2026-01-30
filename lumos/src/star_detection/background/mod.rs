//! Background estimation for star detection.
//!
//! Estimates the sky background using a tiled approach with sigma-clipped
//! statistics, then bilinearly interpolates to create a smooth background map.
//!
//! Uses SIMD acceleration when available for statistics computation.

#[cfg(test)]
pub mod bench;

#[cfg(test)]
mod tests;

mod simd;

use super::constants::{self, ROWS_PER_CHUNK};
use crate::common::Buffer2;
use rayon::prelude::*;

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

/// Tile statistics computed during background estimation.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TileStats {
    median: f32,
    sigma: f32,
}

/// Tile grid with precomputed centers for interpolation.
struct TileGrid {
    stats: Vec<TileStats>,
    centers_x: Vec<f32>,
    centers_y: Vec<f32>,
    tiles_x: usize,
    tiles_y: usize,
}

impl TileGrid {
    #[inline]
    fn get(&self, tx: usize, ty: usize) -> TileStats {
        self.stats[ty * self.tiles_x + tx]
    }

    /// Apply median filter to the tile grid statistics.
    ///
    /// This makes the background estimation more robust to bright stars by
    /// replacing each tile's statistics with the median of its 3x3 neighborhood.
    fn apply_median_filter(&mut self) {
        if self.tiles_x < 3 || self.tiles_y < 3 {
            return; // Not enough tiles for filtering
        }

        let filtered_stats: Vec<TileStats> = (0..self.tiles_y * self.tiles_x)
            .into_par_iter()
            .map(|idx| {
                let ty = idx / self.tiles_x;
                let tx = idx % self.tiles_x;

                // Gather 3x3 neighborhood values
                let mut medians = [0.0f32; 9];
                let mut sigmas = [0.0f32; 9];
                let mut count = 0;

                for dy in -1i32..=1 {
                    for dx in -1i32..=1 {
                        let nx = tx as i32 + dx;
                        let ny = ty as i32 + dy;

                        if nx >= 0
                            && nx < self.tiles_x as i32
                            && ny >= 0
                            && ny < self.tiles_y as i32
                        {
                            let neighbor = self.get(nx as usize, ny as usize);
                            medians[count] = neighbor.median;
                            sigmas[count] = neighbor.sigma;
                            count += 1;
                        }
                    }
                }

                // Compute median of neighborhoods
                let filtered_median = median_of_slice(&mut medians[..count]);
                let filtered_sigma = median_of_slice(&mut sigmas[..count]);

                TileStats {
                    median: filtered_median,
                    sigma: filtered_sigma,
                }
            })
            .collect();

        self.stats = filtered_stats;
    }
}

/// Compute median of a small slice (up to 9 elements).
#[inline]
fn median_of_slice(values: &mut [f32]) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = values.len() / 2;
    if values.len().is_multiple_of(2) {
        (values[mid - 1] + values[mid]) * 0.5
    } else {
        values[mid]
    }
}

/// Estimate background using tiled sigma-clipped statistics.
///
/// # Algorithm
///
/// 1. Divide image into tiles of size `tile_size × tile_size`
/// 2. For each tile, compute sigma-clipped median and standard deviation
/// 3. Bilinearly interpolate between tile centers to get per-pixel values
///
/// # Arguments
/// * `pixels` - Grayscale image buffer
/// * `tile_size` - Size of tiles (typically 32-128 pixels)
pub fn estimate_background(pixels: &Buffer2<f32>, tile_size: usize) -> BackgroundMap {
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

    let tiles_x = width.div_ceil(tile_size);
    let tiles_y = height.div_ceil(tile_size);
    let max_tile_pixels = tile_size * tile_size;

    // Compute statistics for each tile in parallel with per-thread scratch buffers
    let tile_stats: Vec<TileStats> = (0..tiles_y * tiles_x)
        .into_par_iter()
        .map_init(
            || {
                (
                    Vec::with_capacity(max_tile_pixels),
                    Vec::with_capacity(max_tile_pixels),
                )
            },
            |(values_buf, deviations_buf), idx| {
                let ty = idx / tiles_x;
                let tx = idx % tiles_x;

                let x_start = tx * tile_size;
                let y_start = ty * tile_size;
                let x_end = (x_start + tile_size).min(width);
                let y_end = (y_start + tile_size).min(height);

                compute_tile_stats(
                    pixels,
                    width,
                    x_start,
                    x_end,
                    y_start,
                    y_end,
                    values_buf,
                    deviations_buf,
                )
            },
        )
        .collect();

    // Build tile grid with precomputed centers
    let mut grid = TileGrid {
        stats: tile_stats,
        centers_x: (0..tiles_x)
            .map(|tx| {
                let x_start = tx * tile_size;
                let x_end = (x_start + tile_size).min(width);
                (x_start + x_end) as f32 * 0.5
            })
            .collect(),
        centers_y: (0..tiles_y)
            .map(|ty| {
                let y_start = ty * tile_size;
                let y_end = (y_start + tile_size).min(height);
                (y_start + y_end) as f32 * 0.5
            })
            .collect(),
        tiles_x,
        tiles_y,
    };

    // Apply median filter to tile grid to reject bright star contamination
    grid.apply_median_filter();

    // Allocate output buffers
    let mut background = vec![0.0f32; width * height];
    let mut noise = vec![0.0f32; width * height];

    background
        .par_chunks_mut(width * ROWS_PER_CHUNK)
        .zip(noise.par_chunks_mut(width * ROWS_PER_CHUNK))
        .enumerate()
        .for_each(|(chunk_idx, (bg_chunk, noise_chunk))| {
            let y_start = chunk_idx * ROWS_PER_CHUNK;
            let rows_in_chunk = bg_chunk.len() / width;

            for local_y in 0..rows_in_chunk {
                let y = y_start + local_y;
                let row_offset = local_y * width;
                let bg_row = &mut bg_chunk[row_offset..row_offset + width];
                let noise_row = &mut noise_chunk[row_offset..row_offset + width];

                interpolate_row(bg_row, noise_row, y, &grid);
            }
        });

    BackgroundMap {
        background: Buffer2::new(width, height, background),
        noise: Buffer2::new(width, height, noise),
    }
}

/// Compute sigma-clipped statistics for a single tile using provided scratch buffers.
#[allow(clippy::too_many_arguments)]
fn compute_tile_stats(
    pixels: &[f32],
    width: usize,
    x_start: usize,
    x_end: usize,
    y_start: usize,
    y_end: usize,
    values: &mut Vec<f32>,
    deviations: &mut Vec<f32>,
) -> TileStats {
    // Clear and fill values buffer
    values.clear();
    for y in y_start..y_end {
        let row_start = y * width + x_start;
        values.extend_from_slice(&pixels[row_start..row_start + (x_end - x_start)]);
    }

    // Sigma-clipped statistics (3 iterations, 3-sigma clip)
    sigma_clipped_stats(values, deviations, 3.0, 3)
}

/// Compute sigma-clipped tile statistics using the shared implementation.
#[inline]
pub(crate) fn sigma_clipped_stats(
    values: &mut [f32],
    deviations: &mut Vec<f32>,
    kappa: f32,
    iterations: usize,
) -> TileStats {
    let (median, sigma) =
        constants::sigma_clipped_median_mad(values, deviations, kappa, iterations);
    TileStats { median, sigma }
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
    let ty0 = find_lower_tile(fy, &grid.centers_y);
    let ty1 = (ty0 + 1).min(grid.tiles_y - 1);

    let wy = if ty1 != ty0 {
        ((fy - grid.centers_y[ty0]) / (grid.centers_y[ty1] - grid.centers_y[ty0])).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let wy_inv = 1.0 - wy;

    // Process row in segments between tile center X boundaries
    let mut x = 0usize;

    for tx0 in 0..grid.tiles_x {
        let tx1 = (tx0 + 1).min(grid.tiles_x - 1);

        // Segment runs from current x to next tile center (or end of row)
        let segment_end = if tx0 + 1 < grid.tiles_x {
            // Segment ends at next tile center
            (grid.centers_x[tx0 + 1].floor() as usize).min(width)
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

        if tx1 != tx0 {
            // Interpolation needed - use SIMD-accelerated version
            let inv_dx = 1.0 / (grid.centers_x[tx1] - grid.centers_x[tx0]);
            let x_offset = grid.centers_x[tx0];
            let wx_start = (x as f32 - x_offset) * inv_dx;
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

/// Find the tile index whose center is at or before the given position.
#[inline]
fn find_lower_tile(pos: f32, centers: &[f32]) -> usize {
    // Linear search is efficient for small tile counts (typically < 20)
    for i in (0..centers.len()).rev() {
        if centers[i] <= pos {
            return i;
        }
    }
    0
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
    /// Automatically chooses between single-pass and iterative estimation
    /// based on the `iterations` field. If `iterations > 0`, uses iterative
    /// refinement which is better for crowded fields.
    pub fn estimate(&self, pixels: &Buffer2<f32>) -> BackgroundMap {
        if self.iterations > 0 {
            estimate_background_iterative(pixels, self)
        } else {
            estimate_background(pixels, self.tile_size)
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
            "tile_size must be in [16, 256], got {}",
            self.tile_size
        );
    }
}

/// Estimate background with iterative refinement.
///
/// This implements the SExtractor-style iterative approach:
/// 1. Estimate initial background
/// 2. Detect pixels above threshold (potential objects)
/// 3. Create dilated mask of detected regions
/// 4. Re-estimate background excluding masked pixels
/// 5. Repeat for specified iterations
///
/// This provides cleaner background maps for crowded fields by excluding
/// stars and other bright objects from the estimation.
///
/// # Arguments
/// * `pixels` - Grayscale image buffer
/// * `tile_size` - Size of tiles for background estimation
/// * `config` - Iterative refinement configuration
pub fn estimate_background_iterative(
    pixels: &Buffer2<f32>,
    config: &BackgroundConfig,
) -> BackgroundMap {
    // Start with initial background estimate
    let mut background = estimate_background(pixels, config.tile_size);

    // Pre-allocate mask buffers for reuse across iterations
    let width = pixels.width();
    let height = pixels.height();
    let mut mask = Buffer2::new_filled(width, height, false);
    let mut scratch = Buffer2::new_filled(width, height, false);

    for _iter in 0..config.iterations {
        // Create mask of pixels above threshold
        create_object_mask(
            pixels,
            &background,
            config.detection_sigma,
            config.mask_dilation,
            &mut mask,
            &mut scratch,
        );

        // Re-estimate background with masked pixels excluded
        estimate_background_masked(
            pixels,
            config.tile_size,
            &mask,
            config.min_unmasked_fraction,
            &mut background,
        );
    }

    background
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
    // Initial mask: pixels above threshold
    for (i, ((&px, &bg), &noise)) in pixels
        .iter()
        .zip(background.background.iter())
        .zip(background.noise.iter())
        .enumerate()
    {
        let threshold = bg + detection_sigma * noise.max(1e-6);
        output.pixels_mut()[i] = px > threshold;
    }

    // Dilate mask to cover object wings
    if dilation_radius > 0 {
        constants::dilate_mask(output, dilation_radius, scratch);
        output.copy_from(scratch);
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

    let tiles_x = width.div_ceil(tile_size);
    let tiles_y = height.div_ceil(tile_size);
    let max_tile_pixels = tile_size * tile_size;
    let min_pixels = (max_tile_pixels as f32 * min_unmasked_fraction) as usize;

    // Compute statistics for each tile with masking
    let tile_stats: Vec<TileStats> = (0..tiles_y * tiles_x)
        .into_par_iter()
        .map_init(
            || {
                (
                    Vec::with_capacity(max_tile_pixels),
                    Vec::with_capacity(max_tile_pixels),
                )
            },
            |(values_buf, deviations_buf), idx| {
                let ty = idx / tiles_x;
                let tx = idx % tiles_x;

                let x_start = tx * tile_size;
                let y_start = ty * tile_size;
                let x_end = (x_start + tile_size).min(width);
                let y_end = (y_start + tile_size).min(height);

                compute_tile_stats_masked(
                    pixels,
                    mask,
                    x_start,
                    x_end,
                    y_start,
                    y_end,
                    min_pixels,
                    values_buf,
                    deviations_buf,
                )
            },
        )
        .collect();

    // Build tile grid
    let mut grid = TileGrid {
        stats: tile_stats,
        centers_x: (0..tiles_x)
            .map(|tx| {
                let x_start = tx * tile_size;
                let x_end = (x_start + tile_size).min(width);
                (x_start + x_end) as f32 * 0.5
            })
            .collect(),
        centers_y: (0..tiles_y)
            .map(|ty| {
                let y_start = ty * tile_size;
                let y_end = (y_start + tile_size).min(height);
                (y_start + y_end) as f32 * 0.5
            })
            .collect(),
        tiles_x,
        tiles_y,
    };

    // Apply median filter
    grid.apply_median_filter();

    // Allocate output buffers
    let mut background = vec![0.0f32; width * height];
    let mut noise = vec![0.0f32; width * height];

    background
        .par_chunks_mut(width * ROWS_PER_CHUNK)
        .zip(noise.par_chunks_mut(width * ROWS_PER_CHUNK))
        .enumerate()
        .for_each(|(chunk_idx, (bg_chunk, noise_chunk))| {
            let y_start = chunk_idx * ROWS_PER_CHUNK;
            let rows_in_chunk = bg_chunk.len() / width;

            for local_y in 0..rows_in_chunk {
                let y = y_start + local_y;
                let row_offset = local_y * width;
                let bg_row = &mut bg_chunk[row_offset..row_offset + width];
                let noise_row = &mut noise_chunk[row_offset..row_offset + width];

                interpolate_row(bg_row, noise_row, y, &grid);
            }
        });

    output.background = Buffer2::new(width, height, background);
    output.noise = Buffer2::new(width, height, noise);
}

/// Compute tile statistics with masked pixels excluded.
#[allow(clippy::too_many_arguments)]
fn compute_tile_stats_masked(
    pixels: &Buffer2<f32>,
    mask: &Buffer2<bool>,
    x_start: usize,
    x_end: usize,
    y_start: usize,
    y_end: usize,
    min_pixels: usize,
    values: &mut Vec<f32>,
    deviations: &mut Vec<f32>,
) -> TileStats {
    let width = pixels.width();

    // Collect unmasked pixels
    values.clear();
    for y in y_start..y_end {
        let row_start = y * width;
        for x in x_start..x_end {
            let idx = row_start + x;
            if !mask[idx] {
                values.push(pixels[idx]);
            }
        }
    }

    // If too few unmasked pixels, fall back to all pixels (unmasked)
    if values.len() < min_pixels {
        values.clear();
        for y in y_start..y_end {
            let row_start = y * width + x_start;
            values.extend_from_slice(&pixels[row_start..row_start + (x_end - x_start)]);
        }
    }

    sigma_clipped_stats(values, deviations, 3.0, 3)
}
