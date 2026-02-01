//! Tile grid for background estimation interpolation.

use crate::common::{BitBuffer2, Buffer2};
use crate::math::median_f32_mut;
use crate::star_detection::common::sigma_clipped_median_mad;
use rayon::prelude::*;

// ============================================================================
// Constants
// ============================================================================

/// Maximum samples per tile for statistics computation.
const MAX_TILE_SAMPLES: usize = 1024;

/// Number of sigma-clipping iterations.
const SIGMA_CLIP_ITERATIONS: usize = 2;

// ============================================================================
// Types
// ============================================================================

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

// ============================================================================
// TileGrid implementation
// ============================================================================

impl TileGrid {
    pub fn new(pixels: &Buffer2<f32>, tile_size: usize) -> Self {
        Self::new_with_mask(pixels, tile_size, None, 0)
    }

    pub fn new_with_mask(
        pixels: &Buffer2<f32>,
        tile_size: usize,
        mask: Option<&BitBuffer2>,
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

    // ------------------------------------------------------------------------
    // Accessors
    // ------------------------------------------------------------------------

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

    #[inline]
    pub fn center_x(&self, tx: usize) -> f32 {
        let x_start = tx * self.tile_size;
        let x_end = (x_start + self.tile_size).min(self.width);
        (x_start + x_end) as f32 * 0.5
    }

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

        // Binary search for largest tile index with center <= pos
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

    // ------------------------------------------------------------------------
    // Statistics computation
    // ------------------------------------------------------------------------

    fn fill_tile_stats(
        &mut self,
        pixels: &Buffer2<f32>,
        mask: Option<&BitBuffer2>,
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
                |(values, deviations), (idx, out)| {
                    let tx = idx % tiles_x;
                    let ty = idx / tiles_x;

                    let x_start = tx * tile_size;
                    let y_start = ty * tile_size;
                    let x_end = (x_start + tile_size).min(width);
                    let y_end = (y_start + tile_size).min(height);

                    *out = compute_tile_stats(
                        pixels, mask, x_start, x_end, y_start, y_end, min_pixels, values,
                        deviations,
                    );
                },
            );
    }

    fn apply_median_filter(&mut self) {
        let tiles_x = self.tiles_x();
        let tiles_y = self.tiles_y();

        if tiles_x < 3 || tiles_y < 3 {
            return;
        }

        let src = self.stats.pixels();
        let mut dst: Buffer2<TileStats> = Buffer2::new_default(tiles_x, tiles_y);

        dst.pixels_mut()
            .par_iter_mut()
            .enumerate()
            .for_each(|(idx, out)| {
                let tx = idx % tiles_x;
                let ty = idx / tiles_x;

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

                out.median = median_f32_mut(&mut medians[..count]);
                out.sigma = median_f32_mut(&mut sigmas[..count]);
            });

        std::mem::swap(&mut self.stats, &mut dst);
    }
}

// ============================================================================
// Pixel collection helpers
// ============================================================================

/// Compute sigma-clipped statistics for a tile region.
#[allow(clippy::too_many_arguments)]
fn compute_tile_stats(
    pixels: &Buffer2<f32>,
    mask: Option<&BitBuffer2>,
    x_start: usize,
    x_end: usize,
    y_start: usize,
    y_end: usize,
    min_pixels: usize,
    values: &mut Vec<f32>,
    deviations: &mut Vec<f32>,
) -> TileStats {
    values.clear();

    let width = pixels.width();
    let tile_pixels = (x_end - x_start) * (y_end - y_start);

    match mask {
        Some(m) => {
            collect_unmasked_pixels(pixels, m, x_start, x_end, y_start, y_end, width, values);

            if values.len() < min_pixels {
                values.clear();
                collect_sampled_pixels(pixels, x_start, x_end, y_start, y_end, width, values);
            } else if values.len() > MAX_TILE_SAMPLES {
                subsample_in_place(values, MAX_TILE_SAMPLES);
            }
        }
        None => {
            if tile_pixels <= MAX_TILE_SAMPLES {
                collect_all_pixels(pixels, x_start, x_end, y_start, y_end, width, values);
            } else {
                collect_sampled_pixels(pixels, x_start, x_end, y_start, y_end, width, values);
            }
        }
    }

    if values.is_empty() {
        return TileStats::default();
    }

    let (median, sigma) = sigma_clipped_median_mad(values, deviations, 3.0, SIGMA_CLIP_ITERATIONS);
    TileStats { median, sigma }
}

/// Collect all pixels from a tile region.
#[inline]
fn collect_all_pixels(
    pixels: &Buffer2<f32>,
    x_start: usize,
    x_end: usize,
    y_start: usize,
    y_end: usize,
    width: usize,
    values: &mut Vec<f32>,
) {
    let tile_width = x_end - x_start;
    for y in y_start..y_end {
        let row_start = y * width + x_start;
        values.extend_from_slice(&pixels[row_start..row_start + tile_width]);
    }
}

/// Collect sampled pixels using strided access (~MAX_TILE_SAMPLES pixels).
#[inline]
fn collect_sampled_pixels(
    pixels: &Buffer2<f32>,
    x_start: usize,
    x_end: usize,
    y_start: usize,
    y_end: usize,
    width: usize,
    values: &mut Vec<f32>,
) {
    let tile_pixels = (x_end - x_start) * (y_end - y_start);
    let stride = ((tile_pixels / MAX_TILE_SAMPLES).max(1) as f32)
        .sqrt()
        .ceil() as usize;

    for y in (y_start..y_end).step_by(stride) {
        let row_start = y * width;
        for x in (x_start..x_end).step_by(stride) {
            values.push(pixels[row_start + x]);
        }
    }
}

/// Collect unmasked pixels using word-level bit operations.
#[inline]
#[allow(clippy::too_many_arguments)]
fn collect_unmasked_pixels(
    pixels: &Buffer2<f32>,
    mask: &BitBuffer2,
    x_start: usize,
    x_end: usize,
    y_start: usize,
    y_end: usize,
    width: usize,
    values: &mut Vec<f32>,
) {
    let mask_words = mask.words();
    let words_per_row = mask.words_per_row();

    for y in y_start..y_end {
        let row_start = y * width;
        let word_row_start = y * words_per_row;
        let mut x = x_start;

        while x < x_end {
            let word_idx = x / 64;
            let bit_offset = x % 64;
            let mask_word = mask_words[word_row_start + word_idx];

            let bits_in_word = 64 - bit_offset;
            let bits_to_process = bits_in_word.min(x_end - x);

            let relevant_bits = if bits_to_process == 64 {
                !0u64
            } else {
                ((1u64 << bits_to_process) - 1) << bit_offset
            };

            let unmasked = !mask_word & relevant_bits;

            if unmasked != 0 {
                let mut bits = unmasked >> bit_offset;
                let mut local_x = x;

                while bits != 0 && local_x < x_end {
                    let offset = bits.trailing_zeros() as usize;
                    local_x = x + offset;

                    if local_x < x_end {
                        values.push(pixels[row_start + local_x]);
                    }
                    bits &= bits - 1;
                }
            }

            x += bits_to_process;
        }
    }
}

/// Subsample a vector in place to approximately target_size elements.
#[inline]
fn subsample_in_place(values: &mut Vec<f32>, target_size: usize) {
    let len = values.len();
    if len <= target_size {
        return;
    }

    let stride = len / target_size;
    let mut write_idx = 0;

    for read_idx in (0..len).step_by(stride) {
        values[write_idx] = values[read_idx];
        write_idx += 1;
        if write_idx >= target_size {
            break;
        }
    }

    values.truncate(write_idx);
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn create_uniform_image(width: usize, height: usize, value: f32) -> Buffer2<f32> {
        Buffer2::new(width, height, vec![value; width * height])
    }

    // --- Construction ---

    #[test]
    fn test_tile_grid_dimensions() {
        let pixels = create_uniform_image(128, 64, 0.5);
        let grid = TileGrid::new(&pixels, 32);

        assert_eq!(grid.tiles_x(), 4);
        assert_eq!(grid.tiles_y(), 2);
    }

    #[test]
    fn test_tile_grid_dimensions_non_divisible() {
        let pixels = create_uniform_image(100, 70, 0.5);
        let grid = TileGrid::new(&pixels, 32);

        assert_eq!(grid.tiles_x(), 4);
        assert_eq!(grid.tiles_y(), 3);
    }

    #[test]
    fn test_tile_grid_uniform_image() {
        let pixels = create_uniform_image(64, 64, 0.3);
        let grid = TileGrid::new(&pixels, 32);

        for ty in 0..grid.tiles_y() {
            for tx in 0..grid.tiles_x() {
                let stats = grid.get(tx, ty);
                assert!((stats.median - 0.3).abs() < 0.01);
                assert!(stats.sigma < 0.01);
            }
        }
    }

    // --- Center computation ---

    #[test]
    fn test_center_x_full_tiles() {
        let pixels = create_uniform_image(128, 64, 0.5);
        let grid = TileGrid::new(&pixels, 32);

        assert!((grid.center_x(0) - 16.0).abs() < 0.01);
        assert!((grid.center_x(1) - 48.0).abs() < 0.01);
        assert!((grid.center_x(2) - 80.0).abs() < 0.01);
        assert!((grid.center_x(3) - 112.0).abs() < 0.01);
    }

    #[test]
    fn test_center_x_partial_tile() {
        let pixels = create_uniform_image(100, 64, 0.5);
        let grid = TileGrid::new(&pixels, 32);

        assert!((grid.center_x(3) - 98.0).abs() < 0.01);
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

        assert!((grid.center_y(3) - 98.0).abs() < 0.01);
    }

    // --- find_lower_tile_y ---

    #[test]
    fn test_find_lower_tile_y_exact_center() {
        let pixels = create_uniform_image(64, 128, 0.5);
        let grid = TileGrid::new(&pixels, 32);

        assert_eq!(grid.find_lower_tile_y(16.0), 0);
        assert_eq!(grid.find_lower_tile_y(48.0), 1);
        assert_eq!(grid.find_lower_tile_y(80.0), 2);
        assert_eq!(grid.find_lower_tile_y(112.0), 3);
    }

    #[test]
    fn test_find_lower_tile_y_between_centers() {
        let pixels = create_uniform_image(64, 128, 0.5);
        let grid = TileGrid::new(&pixels, 32);

        assert_eq!(grid.find_lower_tile_y(30.0), 0);
        assert_eq!(grid.find_lower_tile_y(60.0), 1);
        assert_eq!(grid.find_lower_tile_y(100.0), 2);
    }

    #[test]
    fn test_find_lower_tile_y_before_first_center() {
        let pixels = create_uniform_image(64, 128, 0.5);
        let grid = TileGrid::new(&pixels, 32);

        assert_eq!(grid.find_lower_tile_y(0.0), 0);
        assert_eq!(grid.find_lower_tile_y(10.0), 0);
    }

    #[test]
    fn test_find_lower_tile_y_after_last_center() {
        let pixels = create_uniform_image(64, 128, 0.5);
        let grid = TileGrid::new(&pixels, 32);

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

    // --- Mask handling ---

    #[test]
    fn test_tile_grid_with_mask_excludes_masked() {
        let width = 64;
        let height = 64;
        let mut data = vec![0.2; width * height];

        for y in 0..32 {
            for x in 0..32 {
                data[y * width + x] = 0.8;
            }
        }

        let pixels = Buffer2::new(width, height, data);

        let mut mask = BitBuffer2::new_filled(width, height, false);
        for y in 0..32 {
            for x in 0..32 {
                mask.set_xy(x, y, true);
            }
        }

        let grid = TileGrid::new_with_mask(&pixels, 32, Some(&mask), 100);

        let stats_11 = grid.get(1, 1);
        assert!((stats_11.median - 0.2).abs() < 0.05);
    }

    #[test]
    fn test_tile_grid_with_mask_fallback_when_too_few_unmasked() {
        let width = 64;
        let height = 64;
        let mut data = vec![0.5; width * height];
        data[0] = 0.9;

        let pixels = Buffer2::new(width, height, data);

        let mut mask = BitBuffer2::new_filled(width, height, false);
        for y in 0..32 {
            for x in 0..32 {
                if !(x == 0 && y == 0) {
                    mask.set_xy(x, y, true);
                }
            }
        }

        let grid = TileGrid::new_with_mask(&pixels, 32, Some(&mask), 100);

        let stats = grid.get(0, 0);
        assert!((stats.median - 0.5).abs() < 0.05);
    }

    #[test]
    fn test_all_pixels_masked_fallback() {
        let width = 64;
        let height = 64;
        let pixels = create_uniform_image(width, height, 0.4);
        let mask = BitBuffer2::new_filled(width, height, true);

        let grid = TileGrid::new_with_mask(&pixels, 32, Some(&mask), 100);

        let stats = grid.get(0, 0);
        assert!((stats.median - 0.4).abs() < 0.05);
    }

    #[test]
    fn test_no_mask_same_as_none() {
        let pixels = create_uniform_image(64, 64, 0.5);

        let grid_none = TileGrid::new(&pixels, 32);
        let grid_empty = TileGrid::new_with_mask(&pixels, 32, None, 0);

        for ty in 0..grid_none.tiles_y() {
            for tx in 0..grid_none.tiles_x() {
                let s1 = grid_none.get(tx, ty);
                let s2 = grid_empty.get(tx, ty);
                assert!((s1.median - s2.median).abs() < 0.001);
            }
        }
    }

    // --- Median filter ---

    #[test]
    fn test_median_filter_uniform_unchanged() {
        let pixels = create_uniform_image(128, 128, 0.4);
        let grid = TileGrid::new(&pixels, 32);

        for ty in 0..grid.tiles_y() {
            for tx in 0..grid.tiles_x() {
                let stats = grid.get(tx, ty);
                assert!((stats.median - 0.4).abs() < 0.01);
            }
        }
    }

    #[test]
    fn test_median_filter_rejects_outlier_tile() {
        let width = 128;
        let height = 128;
        let mut data = vec![0.3; width * height];

        for y in 32..64 {
            for x in 32..64 {
                data[y * width + x] = 0.9;
            }
        }

        let pixels = Buffer2::new(width, height, data);
        let grid = TileGrid::new(&pixels, 32);

        let center_stats = grid.get(1, 1);
        assert!((center_stats.median - 0.3).abs() < 0.1);
    }

    #[test]
    fn test_median_filter_skipped_for_small_grid() {
        let pixels = create_uniform_image(64, 64, 0.5);
        let grid = TileGrid::new(&pixels, 32);

        assert_eq!(grid.tiles_x(), 2);
        assert_eq!(grid.tiles_y(), 2);

        let stats = grid.get(0, 0);
        assert!((stats.median - 0.5).abs() < 0.01);
    }

    // --- Edge cases ---

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

        let tl = grid.get(0, 0);
        let br = grid.get(1, 1);
        assert!(br.median > tl.median);
    }

    #[test]
    fn test_debug_impl() {
        let pixels = create_uniform_image(64, 64, 0.5);
        let grid = TileGrid::new(&pixels, 32);

        let debug_str = format!("{:?}", grid);
        assert!(debug_str.contains("TileGrid"));
    }

    #[test]
    fn test_image_smaller_than_tile() {
        let pixels = create_uniform_image(20, 20, 0.7);
        let grid = TileGrid::new(&pixels, 64);

        assert_eq!(grid.tiles_x(), 1);
        assert_eq!(grid.tiles_y(), 1);

        let stats = grid.get(0, 0);
        assert!((stats.median - 0.7).abs() < 0.01);
        assert!((grid.center_x(0) - 10.0).abs() < 0.01);
        assert!((grid.center_y(0) - 10.0).abs() < 0.01);
    }

    #[test]
    fn test_large_tile_size() {
        let pixels = create_uniform_image(100, 50, 0.3);
        let grid = TileGrid::new(&pixels, 200);

        assert_eq!(grid.tiles_x(), 1);
        assert_eq!(grid.tiles_y(), 1);

        let stats = grid.get(0, 0);
        assert!((stats.median - 0.3).abs() < 0.01);
    }

    #[test]
    fn test_tile_grid_very_wide_image() {
        let pixels = create_uniform_image(1000, 10, 0.5);
        let grid = TileGrid::new(&pixels, 64);

        assert_eq!(grid.tiles_x(), 16);
        assert_eq!(grid.tiles_y(), 1);

        for tx in 0..grid.tiles_x() {
            let stats = grid.get(tx, 0);
            assert!((stats.median - 0.5).abs() < 0.01);
        }
    }

    #[test]
    fn test_tile_grid_very_tall_image() {
        let pixels = create_uniform_image(10, 1000, 0.5);
        let grid = TileGrid::new(&pixels, 64);

        assert_eq!(grid.tiles_x(), 1);
        assert_eq!(grid.tiles_y(), 16);

        for ty in 0..grid.tiles_y() {
            let stats = grid.get(0, ty);
            assert!((stats.median - 0.5).abs() < 0.01);
        }
    }
}
