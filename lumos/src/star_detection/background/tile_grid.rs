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
#[derive(Debug)]
pub(super) struct TileGrid {
    stats: Buffer2<TileStats>,
    tile_size: usize,
    width: usize,
    height: usize,
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

        let mut grid = Self {
            stats: Buffer2::new_default(tiles_x, tiles_y),
            tile_size,
            width,
            height,
        };

        grid.fill_tile_stats(pixels, mask, min_pixels);
        grid.apply_median_filter();

        grid
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

    /// Compute the X center for a tile column.
    #[inline]
    pub fn center_x(&self, tx: usize) -> f32 {
        let x_start = tx * self.tile_size;
        let x_end = (x_start + self.tile_size).min(self.width);
        (x_start + x_end) as f32 * 0.5
    }

    /// Compute the Y center for a tile row.
    #[inline]
    pub fn center_y(&self, ty: usize) -> f32 {
        let y_start = ty * self.tile_size;
        let y_end = (y_start + self.tile_size).min(self.height);
        (y_start + y_end) as f32 * 0.5
    }

    /// Find the tile index whose center is at or before the given Y position.
    #[inline]
    pub fn find_lower_tile_y(&self, pos: f32) -> usize {
        let tiles_y = self.tiles_y();
        if tiles_y == 0 {
            return 0;
        }

        // Binary search for the largest tile index whose center <= pos
        let mut lo = 0;
        let mut hi = tiles_y;
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if self.center_y(mid) <= pos {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }

        lo.saturating_sub(1)
    }

    fn fill_tile_stats(
        &mut self,
        pixels: &Buffer2<f32>,
        mask: Option<&Buffer2<bool>>,
        min_pixels: usize,
    ) {
        let tiles_x = self.tiles_x();
        let tile_size = self.tile_size;
        let width = self.width;
        let height = self.height;
        let max_tile_pixels = tile_size * tile_size;

        self.stats
            .pixels_mut()
            .par_iter_mut()
            .enumerate()
            .for_each_init(
                || {
                    (
                        Vec::with_capacity(max_tile_pixels),
                        Vec::with_capacity(max_tile_pixels),
                    )
                },
                |(values_buf, deviations_buf), (idx, out)| {
                    let ty = idx / tiles_x;
                    let tx = idx % tiles_x;

                    let x_start = tx * tile_size;
                    let y_start = ty * tile_size;
                    let x_end = (x_start + tile_size).min(width);
                    let y_end = (y_start + tile_size).min(height);

                    *out = Self::compute_tile_stats(
                        pixels,
                        mask,
                        x_start,
                        x_end,
                        y_start,
                        y_end,
                        min_pixels,
                        values_buf,
                        deviations_buf,
                    );
                },
            );
    }

    /// Apply median filter to the tile grid statistics.
    ///
    /// This makes the background estimation more robust to bright stars by
    /// replacing each tile's statistics with the median of its 3x3 neighborhood.
    fn apply_median_filter(&mut self) {
        let tiles_x = self.tiles_x();
        let tiles_y = self.tiles_y();

        if tiles_x < 3 || tiles_y < 3 {
            return; // Not enough tiles for filtering
        }

        let src = self.stats.pixels();
        let mut dst: Buffer2<TileStats> = Buffer2::new_default(tiles_x, tiles_y);

        dst.pixels_mut()
            .par_iter_mut()
            .enumerate()
            .for_each(|(idx, out)| {
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
                            let neighbor = src[ny as usize * tiles_x + nx as usize];
                            medians[count] = neighbor.median;
                            sigmas[count] = neighbor.sigma;
                            count += 1;
                        }
                    }
                }

                // Compute median of neighborhoods
                out.median = median_f32_mut(&mut medians[..count]);
                out.sigma = median_f32_mut(&mut sigmas[..count]);
            });

        std::mem::swap(&mut self.stats, &mut dst);
    }
}
