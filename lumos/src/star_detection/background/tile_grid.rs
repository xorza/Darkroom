//! Tile grid for background estimation interpolation.

use crate::common::Buffer2;
use crate::math::median_f32_mut;
use crate::star_detection::common::sigma_clipped_median_mad;
use rayon::prelude::*;

/// Tile statistics computed during background estimation.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct TileStats {
    pub median: f32,
    pub sigma: f32,
}

/// Tile grid with precomputed centers for interpolation.
pub(super) struct TileGrid {
    stats: Buffer2<TileStats>,
    /// X center for each tile column (one per column).
    pub centers_x: Vec<f32>,
    /// Y center for each tile row (one per row).
    pub centers_y: Vec<f32>,
}

impl TileGrid {
    pub fn new(pixels: &Buffer2<f32>, tile_size: usize) -> Self {
        Self::new_with_mask(pixels, tile_size, None, 0)
    }

    pub fn new_with_mask(
        pixels: &Buffer2<f32>,
        tile_size: usize,
        mask: Option<&Buffer2<bool>>,
        min_pixels: usize,
    ) -> Self {
        let width = pixels.width();
        let height = pixels.height();

        let tiles_x = width.div_ceil(tile_size);
        let tiles_y = height.div_ceil(tile_size);
        let max_tile_pixels = tile_size * tile_size;

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

                    Self::compute_tile_stats(
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

        Self {
            stats: Buffer2::new(tiles_x, tiles_y, tile_stats),
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
        }
    }

    /// Compute sigma-clipped statistics for a single tile using provided scratch buffers.
    /// If mask is provided, excludes masked pixels (falls back to all pixels if too few unmasked).
    #[allow(clippy::too_many_arguments)]
    fn compute_tile_stats(
        pixels: &Buffer2<f32>,
        mask: Option<&Buffer2<bool>>,
        x_start: usize,
        x_end: usize,
        y_start: usize,
        y_end: usize,
        min_pixels: usize,
        values: &mut Vec<f32>,
        deviations: &mut Vec<f32>,
    ) -> TileStats {
        let width = pixels.width();
        values.clear();

        if let Some(mask) = mask {
            // Collect unmasked pixels
            for y in y_start..y_end {
                let row_start = y * width;
                for x in x_start..x_end {
                    let idx = row_start + x;
                    if !mask[idx] {
                        values.push(pixels[idx]);
                    }
                }
            }

            // If too few unmasked pixels, fall back to all pixels
            if values.len() < min_pixels {
                values.clear();
                for y in y_start..y_end {
                    let row_start = y * width + x_start;
                    values.extend_from_slice(&pixels[row_start..row_start + (x_end - x_start)]);
                }
            }
        } else {
            // No mask - collect all pixels
            for y in y_start..y_end {
                let row_start = y * width + x_start;
                values.extend_from_slice(&pixels[row_start..row_start + (x_end - x_start)]);
            }
        }

        // Sigma-clipped statistics (3 iterations, 3-sigma clip)
        let (median, sigma) = sigma_clipped_median_mad(values, deviations, 3.0, 3);
        TileStats { median, sigma }
    }

    #[inline]
    pub fn get(&self, tx: usize, ty: usize) -> TileStats {
        self.stats[(tx, ty)]
    }

    #[inline]
    pub fn tiles_x(&self) -> usize {
        self.stats.width()
    }

    #[inline]
    pub fn tiles_y(&self) -> usize {
        self.stats.height()
    }

    /// Apply median filter to the tile grid statistics.
    ///
    /// This makes the background estimation more robust to bright stars by
    /// replacing each tile's statistics with the median of its 3x3 neighborhood.
    pub fn apply_median_filter(&mut self) {
        let tiles_x = self.tiles_x();
        let tiles_y = self.tiles_y();

        if tiles_x < 3 || tiles_y < 3 {
            return; // Not enough tiles for filtering
        }

        let filtered_stats: Vec<TileStats> = (0..tiles_y * tiles_x)
            .into_par_iter()
            .map(|idx| {
                let ty = idx / tiles_x;
                let tx = idx % tiles_x;

                // Gather 3x3 neighborhood values
                let mut medians = [0.0f32; 9];
                let mut sigmas = [0.0f32; 9];
                let mut count = 0;

                for dy in -1i32..=1 {
                    for dx in -1i32..=1 {
                        let nx = tx as i32 + dx;
                        let ny = ty as i32 + dy;

                        if nx >= 0 && nx < tiles_x as i32 && ny >= 0 && ny < tiles_y as i32 {
                            let neighbor = self.get(nx as usize, ny as usize);
                            medians[count] = neighbor.median;
                            sigmas[count] = neighbor.sigma;
                            count += 1;
                        }
                    }
                }

                // Compute median of neighborhoods
                let filtered_median = median_f32_mut(&mut medians[..count]);
                let filtered_sigma = median_f32_mut(&mut sigmas[..count]);

                TileStats {
                    median: filtered_median,
                    sigma: filtered_sigma,
                }
            })
            .collect();

        self.stats = Buffer2::new(tiles_x, tiles_y, filtered_stats);
    }
}
