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

#[cfg(test)]
mod tests {
    use super::*;

    fn create_uniform_image(width: usize, height: usize, value: f32) -> Buffer2<f32> {
        Buffer2::new(width, height, vec![value; width * height])
    }

    // ==========================================================================
    // TileGrid construction tests
    // ==========================================================================

    #[test]
    fn test_tile_grid_dimensions() {
        let pixels = create_uniform_image(128, 64, 0.5);
        let grid = TileGrid::new(&pixels, 32);

        assert_eq!(grid.tiles_x(), 4); // 128 / 32 = 4
        assert_eq!(grid.tiles_y(), 2); // 64 / 32 = 2
    }

    #[test]
    fn test_tile_grid_dimensions_non_divisible() {
        let pixels = create_uniform_image(100, 70, 0.5);
        let grid = TileGrid::new(&pixels, 32);

        assert_eq!(grid.tiles_x(), 4); // ceil(100 / 32) = 4
        assert_eq!(grid.tiles_y(), 3); // ceil(70 / 32) = 3
    }

    #[test]
    fn test_tile_grid_uniform_image() {
        let pixels = create_uniform_image(64, 64, 0.3);
        let grid = TileGrid::new(&pixels, 32);

        for ty in 0..grid.tiles_y() {
            for tx in 0..grid.tiles_x() {
                let stats = grid.get(tx, ty);
                assert!(
                    (stats.median - 0.3).abs() < 0.01,
                    "Tile ({}, {}) median {} != 0.3",
                    tx,
                    ty,
                    stats.median
                );
                assert!(
                    stats.sigma < 0.01,
                    "Tile ({}, {}) sigma {} should be near zero",
                    tx,
                    ty,
                    stats.sigma
                );
            }
        }
    }

    // ==========================================================================
    // Center computation tests
    // ==========================================================================

    #[test]
    fn test_center_x_full_tiles() {
        let pixels = create_uniform_image(128, 64, 0.5);
        let grid = TileGrid::new(&pixels, 32);

        // Full tiles: center = start + tile_size/2
        assert!((grid.center_x(0) - 16.0).abs() < 0.01); // (0 + 32) / 2 = 16
        assert!((grid.center_x(1) - 48.0).abs() < 0.01); // (32 + 64) / 2 = 48
        assert!((grid.center_x(2) - 80.0).abs() < 0.01); // (64 + 96) / 2 = 80
        assert!((grid.center_x(3) - 112.0).abs() < 0.01); // (96 + 128) / 2 = 112
    }

    #[test]
    fn test_center_x_partial_tile() {
        let pixels = create_uniform_image(100, 64, 0.5);
        let grid = TileGrid::new(&pixels, 32);

        // Last tile is partial: 96 to 100
        assert!((grid.center_x(3) - 98.0).abs() < 0.01); // (96 + 100) / 2 = 98
    }

    #[test]
    fn test_center_y_full_tiles() {
        let pixels = create_uniform_image(64, 128, 0.5);
        let grid = TileGrid::new(&pixels, 32);

        assert!((grid.center_y(0) - 16.0).abs() < 0.01);
        assert!((grid.center_y(1) - 48.0).abs() < 0.01);
        assert!((grid.center_y(2) - 80.0).abs() < 0.01);
        assert!((grid.center_y(3) - 112.0).abs() < 0.01);
    }

    #[test]
    fn test_center_y_partial_tile() {
        let pixels = create_uniform_image(64, 100, 0.5);
        let grid = TileGrid::new(&pixels, 32);

        // Last tile is partial: 96 to 100
        assert!((grid.center_y(3) - 98.0).abs() < 0.01);
    }

    // ==========================================================================
    // find_lower_tile_y tests (binary search)
    // ==========================================================================

    #[test]
    fn test_find_lower_tile_y_exact_center() {
        let pixels = create_uniform_image(64, 128, 0.5);
        let grid = TileGrid::new(&pixels, 32);

        // Exact center positions
        assert_eq!(grid.find_lower_tile_y(16.0), 0);
        assert_eq!(grid.find_lower_tile_y(48.0), 1);
        assert_eq!(grid.find_lower_tile_y(80.0), 2);
        assert_eq!(grid.find_lower_tile_y(112.0), 3);
    }

    #[test]
    fn test_find_lower_tile_y_between_centers() {
        let pixels = create_uniform_image(64, 128, 0.5);
        let grid = TileGrid::new(&pixels, 32);

        // Between centers should return lower tile
        assert_eq!(grid.find_lower_tile_y(30.0), 0); // between 16 and 48
        assert_eq!(grid.find_lower_tile_y(60.0), 1); // between 48 and 80
        assert_eq!(grid.find_lower_tile_y(100.0), 2); // between 80 and 112
    }

    #[test]
    fn test_find_lower_tile_y_before_first_center() {
        let pixels = create_uniform_image(64, 128, 0.5);
        let grid = TileGrid::new(&pixels, 32);

        // Before first center should return 0
        assert_eq!(grid.find_lower_tile_y(0.0), 0);
        assert_eq!(grid.find_lower_tile_y(10.0), 0);
    }

    #[test]
    fn test_find_lower_tile_y_after_last_center() {
        let pixels = create_uniform_image(64, 128, 0.5);
        let grid = TileGrid::new(&pixels, 32);

        // After last center should return last tile
        assert_eq!(grid.find_lower_tile_y(120.0), 3);
        assert_eq!(grid.find_lower_tile_y(1000.0), 3);
    }

    #[test]
    fn test_find_lower_tile_y_single_tile() {
        let pixels = create_uniform_image(32, 32, 0.5);
        let grid = TileGrid::new(&pixels, 32);

        assert_eq!(grid.tiles_y(), 1);
        assert_eq!(grid.find_lower_tile_y(0.0), 0);
        assert_eq!(grid.find_lower_tile_y(16.0), 0);
        assert_eq!(grid.find_lower_tile_y(100.0), 0);
    }

    // ==========================================================================
    // Mask tests
    // ==========================================================================

    #[test]
    fn test_tile_grid_with_mask_excludes_masked() {
        let width = 64;
        let height = 64;
        let mut data = vec![0.2; width * height];

        // Set top-left quadrant to 0.8
        for y in 0..32 {
            for x in 0..32 {
                data[y * width + x] = 0.8;
            }
        }

        let pixels = Buffer2::new(width, height, data);

        // Mask out the top-left quadrant (the 0.8 values)
        let mut mask_data = vec![false; width * height];
        for y in 0..32 {
            for x in 0..32 {
                mask_data[y * width + x] = true;
            }
        }
        let mask = Buffer2::new(width, height, mask_data);

        let grid = TileGrid::new_with_mask(&pixels, 32, Some(&mask), 100);

        // Top-left tile (0,0) has all pixels masked, should fall back to all pixels
        // Other tiles should have median ~0.2
        let stats_11 = grid.get(1, 1);
        assert!(
            (stats_11.median - 0.2).abs() < 0.05,
            "Tile (1,1) median {} should be ~0.2",
            stats_11.median
        );
    }

    #[test]
    fn test_tile_grid_with_mask_fallback_when_too_few_unmasked() {
        let width = 64;
        let height = 64;
        let mut data = vec![0.5; width * height];

        // Put a bright value in top-left tile
        data[0] = 0.9;

        let pixels = Buffer2::new(width, height, data);

        // Mask almost all pixels in top-left tile
        let mut mask_data = vec![false; width * height];
        for y in 0..32 {
            for x in 0..32 {
                if !(x == 0 && y == 0) {
                    mask_data[y * width + x] = true;
                }
            }
        }
        let mask = Buffer2::new(width, height, mask_data);

        // min_pixels = 100, but only 1 pixel unmasked, so should fall back
        let grid = TileGrid::new_with_mask(&pixels, 32, Some(&mask), 100);

        let stats = grid.get(0, 0);
        // Fallback uses all pixels, median should be ~0.5
        assert!(
            (stats.median - 0.5).abs() < 0.05,
            "Tile (0,0) median {} should be ~0.5 after fallback",
            stats.median
        );
    }

    // ==========================================================================
    // Median filter tests
    // ==========================================================================

    #[test]
    fn test_median_filter_uniform_unchanged() {
        let pixels = create_uniform_image(128, 128, 0.4);
        let grid = TileGrid::new(&pixels, 32);

        // All tiles should still be ~0.4 after median filtering
        for ty in 0..grid.tiles_y() {
            for tx in 0..grid.tiles_x() {
                let stats = grid.get(tx, ty);
                assert!(
                    (stats.median - 0.4).abs() < 0.01,
                    "Tile ({}, {}) median {} should be ~0.4",
                    tx,
                    ty,
                    stats.median
                );
            }
        }
    }

    #[test]
    fn test_median_filter_rejects_outlier_tile() {
        let width = 128;
        let height = 128;
        let mut data = vec![0.3; width * height];

        // Make center tile (1,1) very bright
        for y in 32..64 {
            for x in 32..64 {
                data[y * width + x] = 0.9;
            }
        }

        let pixels = Buffer2::new(width, height, data);
        let grid = TileGrid::new(&pixels, 32);

        // Center tile after median filter should be closer to neighbors (~0.3)
        // because median of [0.3, 0.3, 0.3, 0.3, 0.9, 0.3, 0.3, 0.3, 0.3] = 0.3
        let center_stats = grid.get(1, 1);
        assert!(
            (center_stats.median - 0.3).abs() < 0.1,
            "Center tile median {} should be ~0.3 after filtering",
            center_stats.median
        );
    }

    #[test]
    fn test_median_filter_skipped_for_small_grid() {
        // Grid smaller than 3x3 tiles should skip median filter
        let pixels = create_uniform_image(64, 64, 0.5);
        let grid = TileGrid::new(&pixels, 32);

        assert_eq!(grid.tiles_x(), 2);
        assert_eq!(grid.tiles_y(), 2);

        // Should still work, just no filtering applied
        let stats = grid.get(0, 0);
        assert!(
            (stats.median - 0.5).abs() < 0.01,
            "Tile median {} should be ~0.5",
            stats.median
        );
    }

    // ==========================================================================
    // Edge cases
    // ==========================================================================

    #[test]
    fn test_single_tile_image() {
        let pixels = create_uniform_image(32, 32, 0.6);
        let grid = TileGrid::new(&pixels, 32);

        assert_eq!(grid.tiles_x(), 1);
        assert_eq!(grid.tiles_y(), 1);

        let stats = grid.get(0, 0);
        assert!((stats.median - 0.6).abs() < 0.01);

        assert!((grid.center_x(0) - 16.0).abs() < 0.01);
        assert!((grid.center_y(0) - 16.0).abs() < 0.01);
    }

    #[test]
    fn test_tile_stats_with_gradient() {
        let width = 64;
        let height = 64;
        let data: Vec<f32> = (0..height)
            .flat_map(|y| (0..width).map(move |x| (x + y) as f32 / 128.0))
            .collect();

        let pixels = Buffer2::new(width, height, data);
        let grid = TileGrid::new(&pixels, 32);

        // Top-left tile should have lower median than bottom-right
        let tl = grid.get(0, 0);
        let br = grid.get(1, 1);

        assert!(
            br.median > tl.median,
            "Bottom-right median {} should be > top-left {}",
            br.median,
            tl.median
        );
    }

    #[test]
    fn test_debug_impl() {
        let pixels = create_uniform_image(64, 64, 0.5);
        let grid = TileGrid::new(&pixels, 32);

        // Should not panic
        let debug_str = format!("{:?}", grid);
        assert!(debug_str.contains("TileGrid"));
    }
}
