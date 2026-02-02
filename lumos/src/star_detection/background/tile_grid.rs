//! Tile grid for background estimation interpolation.

use crate::common::{BitBuffer2, Buffer2};
use crate::math::median_f32_mut;
use crate::math::sigma_clipped_median_mad;
use crate::star_detection::config::{self, AdaptiveSigmaConfig};
use rayon::prelude::*;

// ============================================================================
// Constants
// ============================================================================

/// Maximum samples per tile for statistics computation.
const MAX_TILE_SAMPLES: usize = 1024;

// ============================================================================
// Types
// ============================================================================

/// Tile statistics computed during background estimation.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct TileStats {
    pub median: f32,
    pub sigma: f32,
    /// Adaptive detection threshold in sigma units.
    /// Computed based on local contrast - higher in nebulous regions.
    pub adaptive_sigma: f32,
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
    /// Create an uninitialized TileGrid with preallocated buffers.
    ///
    /// Call `compute` to fill in the tile statistics.
    pub fn new_uninit(width: usize, height: usize, tile_size: usize) -> Self {
        let tiles_x = width.div_ceil(tile_size);
        let tiles_y = height.div_ceil(tile_size);

        Self {
            stats: Buffer2::new_default(tiles_x, tiles_y),
            tile_size,
            width,
            height,
        }
    }

    /// Compute tile statistics, reusing the existing buffer.
    pub fn compute(
        &mut self,
        pixels: &Buffer2<f32>,
        mask: Option<&BitBuffer2>,
        min_pixels: usize,
        sigma_clip_iterations: usize,
        adaptive_config: Option<AdaptiveSigmaConfig>,
    ) {
        debug_assert_eq!(pixels.width(), self.width);
        debug_assert_eq!(pixels.height(), self.height);

        self.fill_tile_stats(
            pixels,
            mask,
            min_pixels,
            sigma_clip_iterations,
            adaptive_config,
        );
        self.apply_median_filter();
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
        sigma_clip_iterations: usize,
        adaptive_config: Option<AdaptiveSigmaConfig>,
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
                        pixels,
                        mask,
                        x_start,
                        x_end,
                        y_start,
                        y_end,
                        min_pixels,
                        sigma_clip_iterations,
                        adaptive_config,
                        values,
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
                let mut adaptive_sigmas = [0.0f32; 9];
                let mut count = 0;

                for dy in -1i32..=1 {
                    for dx in -1i32..=1 {
                        let nx = tx as i32 + dx;
                        let ny = ty as i32 + dy;

                        if nx >= 0 && nx < tiles_x as i32 && ny >= 0 && ny < tiles_y as i32 {
                            let neighbor = src[ny as usize * tiles_x + nx as usize];
                            medians[count] = neighbor.median;
                            sigmas[count] = neighbor.sigma;
                            adaptive_sigmas[count] = neighbor.adaptive_sigma;
                            count += 1;
                        }
                    }
                }

                out.median = median_f32_mut(&mut medians[..count]);
                out.sigma = median_f32_mut(&mut sigmas[..count]);
                out.adaptive_sigma = median_f32_mut(&mut adaptive_sigmas[..count]);
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
    sigma_clip_iterations: usize,
    adaptive_config: Option<AdaptiveSigmaConfig>,
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

    let (median, sigma) = sigma_clipped_median_mad(values, deviations, 3.0, sigma_clip_iterations);

    // Compute adaptive sigma based on local contrast
    let adaptive_sigma = match adaptive_config {
        Some(config) => compute_adaptive_sigma(values, median, sigma, config),
        None => config::BackgroundConfig::default().sigma_threshold,
    };

    TileStats {
        median,
        sigma,
        adaptive_sigma,
    }
}

/// Compute adaptive detection sigma based on local tile contrast.
///
/// Uses the interquartile range (IQR) normalized by noise as a robust
/// contrast measure. High contrast regions (nebulosity, gradients) get
/// higher sigma thresholds to reduce false positives.
fn compute_adaptive_sigma(
    values: &[f32],
    median: f32,
    sigma: f32,
    config: AdaptiveSigmaConfig,
) -> f32 {
    if values.len() < 4 || sigma < 1e-6 {
        return config.base_sigma;
    }

    // Compute robust contrast metric using coefficient of variation (CV)
    // CV = sigma / |median| measures relative variability
    // For sky background, CV is typically very low (<0.01)
    // For nebulous regions, CV is higher (0.05-0.2+)
    let cv = sigma / median.abs().max(1e-6);

    // Scale contrast to [0, 1] range
    // cv of 0.1 = moderate nebulosity
    // cv of 0.2+ = strong nebulosity
    let contrast_normalized = (cv * config.contrast_factor * 10.0).min(1.0);

    // Interpolate between base_sigma and max_sigma based on contrast
    let adaptive = config.base_sigma + contrast_normalized * (config.max_sigma - config.base_sigma);

    adaptive.clamp(config.base_sigma, config.max_sigma)
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

    /// Number of sigma-clipping iterations for tests.
    const TEST_SIGMA_CLIP_ITERATIONS: usize = 2;

    fn create_uniform_image(width: usize, height: usize, value: f32) -> Buffer2<f32> {
        Buffer2::new(width, height, vec![value; width * height])
    }

    /// Create a TileGrid with default test parameters (no mask, default sigma clip iterations)
    fn make_grid(pixels: &Buffer2<f32>, tile_size: usize) -> TileGrid {
        let mut grid = TileGrid::new_uninit(pixels.width(), pixels.height(), tile_size);
        grid.compute(pixels, None, 0, TEST_SIGMA_CLIP_ITERATIONS, None);
        grid
    }

    /// Create a TileGrid with mask
    fn make_grid_with_mask(
        pixels: &Buffer2<f32>,
        tile_size: usize,
        mask: &BitBuffer2,
        min_pixels: usize,
    ) -> TileGrid {
        let mut grid = TileGrid::new_uninit(pixels.width(), pixels.height(), tile_size);
        grid.compute(
            pixels,
            Some(mask),
            min_pixels,
            TEST_SIGMA_CLIP_ITERATIONS,
            None,
        );
        grid
    }

    // --- Construction ---

    #[test]
    fn test_tile_grid_dimensions() {
        let pixels = create_uniform_image(128, 64, 0.5);
        let grid = make_grid(&pixels, 32);

        assert_eq!(grid.tiles_x(), 4);
        assert_eq!(grid.tiles_y(), 2);
    }

    #[test]
    fn test_tile_grid_dimensions_non_divisible() {
        let pixels = create_uniform_image(100, 70, 0.5);
        let grid = make_grid(&pixels, 32);

        assert_eq!(grid.tiles_x(), 4);
        assert_eq!(grid.tiles_y(), 3);
    }

    #[test]
    fn test_tile_grid_uniform_image() {
        let pixels = create_uniform_image(64, 64, 0.3);
        let grid = make_grid(&pixels, 32);

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
        let grid = make_grid(&pixels, 32);

        assert!((grid.center_x(0) - 16.0).abs() < 0.01);
        assert!((grid.center_x(1) - 48.0).abs() < 0.01);
        assert!((grid.center_x(2) - 80.0).abs() < 0.01);
        assert!((grid.center_x(3) - 112.0).abs() < 0.01);
    }

    #[test]
    fn test_center_x_partial_tile() {
        let pixels = create_uniform_image(100, 64, 0.5);
        let grid = make_grid(&pixels, 32);

        assert!((grid.center_x(3) - 98.0).abs() < 0.01);
    }

    #[test]
    fn test_center_y_full_tiles() {
        let pixels = create_uniform_image(64, 128, 0.5);
        let grid = make_grid(&pixels, 32);

        assert!((grid.center_y(0) - 16.0).abs() < 0.01);
        assert!((grid.center_y(1) - 48.0).abs() < 0.01);
        assert!((grid.center_y(2) - 80.0).abs() < 0.01);
        assert!((grid.center_y(3) - 112.0).abs() < 0.01);
    }

    #[test]
    fn test_center_y_partial_tile() {
        let pixels = create_uniform_image(64, 100, 0.5);
        let grid = make_grid(&pixels, 32);

        assert!((grid.center_y(3) - 98.0).abs() < 0.01);
    }

    // --- find_lower_tile_y ---

    #[test]
    fn test_find_lower_tile_y_exact_center() {
        let pixels = create_uniform_image(64, 128, 0.5);
        let grid = make_grid(&pixels, 32);

        assert_eq!(grid.find_lower_tile_y(16.0), 0);
        assert_eq!(grid.find_lower_tile_y(48.0), 1);
        assert_eq!(grid.find_lower_tile_y(80.0), 2);
        assert_eq!(grid.find_lower_tile_y(112.0), 3);
    }

    #[test]
    fn test_find_lower_tile_y_between_centers() {
        let pixels = create_uniform_image(64, 128, 0.5);
        let grid = make_grid(&pixels, 32);

        assert_eq!(grid.find_lower_tile_y(30.0), 0);
        assert_eq!(grid.find_lower_tile_y(60.0), 1);
        assert_eq!(grid.find_lower_tile_y(100.0), 2);
    }

    #[test]
    fn test_find_lower_tile_y_before_first_center() {
        let pixels = create_uniform_image(64, 128, 0.5);
        let grid = make_grid(&pixels, 32);

        assert_eq!(grid.find_lower_tile_y(0.0), 0);
        assert_eq!(grid.find_lower_tile_y(10.0), 0);
    }

    #[test]
    fn test_find_lower_tile_y_after_last_center() {
        let pixels = create_uniform_image(64, 128, 0.5);
        let grid = make_grid(&pixels, 32);

        assert_eq!(grid.find_lower_tile_y(120.0), 3);
        assert_eq!(grid.find_lower_tile_y(1000.0), 3);
    }

    #[test]
    fn test_find_lower_tile_y_single_tile() {
        let pixels = create_uniform_image(32, 32, 0.5);
        let grid = make_grid(&pixels, 32);

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

        let grid = make_grid_with_mask(&pixels, 32, &mask, 100);

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

        let grid = make_grid_with_mask(&pixels, 32, &mask, 100);

        let stats = grid.get(0, 0);
        assert!((stats.median - 0.5).abs() < 0.05);
    }

    #[test]
    fn test_all_pixels_masked_fallback() {
        let width = 64;
        let height = 64;
        let pixels = create_uniform_image(width, height, 0.4);
        let mask = BitBuffer2::new_filled(width, height, true);

        let grid = make_grid_with_mask(&pixels, 32, &mask, 100);

        let stats = grid.get(0, 0);
        assert!((stats.median - 0.4).abs() < 0.05);
    }

    #[test]
    fn test_no_mask_same_as_none() {
        let pixels = create_uniform_image(64, 64, 0.5);

        let grid_none = make_grid(&pixels, 32);

        let mut grid_empty = TileGrid::new_uninit(64, 64, 32);
        grid_empty.compute(&pixels, None, 0, TEST_SIGMA_CLIP_ITERATIONS, None);

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
        let grid = make_grid(&pixels, 32);

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
        let grid = make_grid(&pixels, 32);

        let center_stats = grid.get(1, 1);
        assert!((center_stats.median - 0.3).abs() < 0.1);
    }

    #[test]
    fn test_median_filter_skipped_for_small_grid() {
        let pixels = create_uniform_image(64, 64, 0.5);
        let grid = make_grid(&pixels, 32);

        assert_eq!(grid.tiles_x(), 2);
        assert_eq!(grid.tiles_y(), 2);

        let stats = grid.get(0, 0);
        assert!((stats.median - 0.5).abs() < 0.01);
    }

    // --- Edge cases ---

    #[test]
    fn test_single_tile_image() {
        let pixels = create_uniform_image(32, 32, 0.6);
        let grid = make_grid(&pixels, 32);

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
        let grid = make_grid(&pixels, 32);

        let tl = grid.get(0, 0);
        let br = grid.get(1, 1);
        assert!(br.median > tl.median);
    }

    #[test]
    fn test_debug_impl() {
        let pixels = create_uniform_image(64, 64, 0.5);
        let grid = make_grid(&pixels, 32);

        let debug_str = format!("{:?}", grid);
        assert!(debug_str.contains("TileGrid"));
    }

    #[test]
    fn test_image_smaller_than_tile() {
        let pixels = create_uniform_image(20, 20, 0.7);
        let grid = make_grid(&pixels, 64);

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
        let grid = make_grid(&pixels, 200);

        assert_eq!(grid.tiles_x(), 1);
        assert_eq!(grid.tiles_y(), 1);

        let stats = grid.get(0, 0);
        assert!((stats.median - 0.3).abs() < 0.01);
    }

    #[test]
    fn test_tile_grid_very_wide_image() {
        let pixels = create_uniform_image(1000, 10, 0.5);
        let grid = make_grid(&pixels, 64);

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
        let grid = make_grid(&pixels, 64);

        assert_eq!(grid.tiles_x(), 1);
        assert_eq!(grid.tiles_y(), 16);

        for ty in 0..grid.tiles_y() {
            let stats = grid.get(0, ty);
            assert!((stats.median - 0.5).abs() < 0.01);
        }
    }

    // --- Helper function tests ---

    #[test]
    fn test_subsample_in_place_no_change_when_small() {
        let mut values = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        subsample_in_place(&mut values, 10);
        assert_eq!(values.len(), 5);
    }

    #[test]
    fn test_subsample_in_place_reduces_size() {
        let mut values: Vec<f32> = (0..1000).map(|i| i as f32).collect();
        subsample_in_place(&mut values, 100);
        assert!(values.len() <= 100);
        // First element should be preserved
        assert_eq!(values[0], 0.0);
    }

    #[test]
    fn test_collect_sampled_pixels_small_tile() {
        let pixels = create_uniform_image(32, 32, 0.5);
        let mut values = Vec::new();
        collect_sampled_pixels(&pixels, 0, 32, 0, 32, 32, &mut values);
        // Small tile should collect all or most pixels
        assert!(values.len() >= 100);
        assert!(values.iter().all(|&v| (v - 0.5).abs() < 0.01));
    }

    #[test]
    fn test_collect_sampled_pixels_large_tile() {
        let pixels = create_uniform_image(256, 256, 0.5);
        let mut values = Vec::new();
        collect_sampled_pixels(&pixels, 0, 256, 0, 256, 256, &mut values);
        // Large tile should sample ~MAX_TILE_SAMPLES
        assert!(values.len() <= MAX_TILE_SAMPLES * 2);
        assert!(values.iter().all(|&v| (v - 0.5).abs() < 0.01));
    }

    #[test]
    fn test_collect_unmasked_pixels_none_masked() {
        let pixels = create_uniform_image(64, 64, 0.5);
        let mask = BitBuffer2::new_filled(64, 64, false);
        let mut values = Vec::new();
        collect_unmasked_pixels(&pixels, &mask, 0, 64, 0, 64, 64, &mut values);
        assert_eq!(values.len(), 64 * 64);
    }

    #[test]
    fn test_collect_unmasked_pixels_all_masked() {
        let pixels = create_uniform_image(64, 64, 0.5);
        let mask = BitBuffer2::new_filled(64, 64, true);
        let mut values = Vec::new();
        collect_unmasked_pixels(&pixels, &mask, 0, 64, 0, 64, 64, &mut values);
        assert_eq!(values.len(), 0);
    }

    #[test]
    fn test_collect_unmasked_pixels_partial_mask() {
        let width = 64;
        let height = 64;
        let pixels = create_uniform_image(width, height, 0.5);

        // Mask every other pixel
        let mut mask = BitBuffer2::new_filled(width, height, false);
        for y in 0..height {
            for x in 0..width {
                if (x + y) % 2 == 0 {
                    mask.set_xy(x, y, true);
                }
            }
        }

        let mut values = Vec::new();
        collect_unmasked_pixels(&pixels, &mask, 0, 64, 0, 64, 64, &mut values);
        // Half pixels should be unmasked
        assert_eq!(values.len(), 64 * 64 / 2);
    }

    #[test]
    fn test_collect_unmasked_pixels_partial_tile() {
        let pixels = create_uniform_image(100, 100, 0.5);
        let mask = BitBuffer2::new_filled(100, 100, false);
        let mut values = Vec::new();
        // Collect from a sub-region not aligned to 64-bit boundaries
        collect_unmasked_pixels(&pixels, &mask, 10, 70, 20, 80, 100, &mut values);
        assert_eq!(values.len(), 60 * 60);
    }

    #[test]
    fn test_tile_with_outliers_sigma_clipped() {
        let width = 64;
        let height = 64;
        let mut data = vec![0.5; width * height];

        // Add some outliers
        for val in data.iter_mut().take(10) {
            *val = 10.0; // Bright outliers
        }

        let pixels = Buffer2::new(width, height, data);
        let grid = make_grid(&pixels, 64);

        let stats = grid.get(0, 0);
        // Median should be close to 0.5 despite outliers
        assert!((stats.median - 0.5).abs() < 0.1);
    }

    #[test]
    fn test_tile_stats_sigma_nonzero_for_varied_data() {
        let width = 64;
        let height = 64;
        // Create data with variation
        let data: Vec<f32> = (0..width * height)
            .map(|i| 0.5 + (i % 10) as f32 * 0.01)
            .collect();

        let pixels = Buffer2::new(width, height, data);
        let grid = make_grid(&pixels, 64);

        let stats = grid.get(0, 0);
        assert!(stats.sigma > 0.0);
    }

    #[test]
    fn test_median_filter_corner_tiles() {
        // Test that corner tiles (with fewer neighbors) are handled correctly
        let pixels = create_uniform_image(128, 128, 0.5);
        let grid = make_grid(&pixels, 32);

        // Corner tiles should still have valid stats
        let corners = [(0, 0), (3, 0), (0, 3), (3, 3)];
        for (tx, ty) in corners {
            let stats = grid.get(tx, ty);
            assert!((stats.median - 0.5).abs() < 0.01);
        }
    }

    #[test]
    fn test_negative_pixel_values() {
        let width = 64;
        let height = 64;
        let data = vec![-0.5; width * height];

        let pixels = Buffer2::new(width, height, data);
        let grid = make_grid(&pixels, 32);

        let stats = grid.get(0, 0);
        assert!((stats.median - (-0.5)).abs() < 0.01);
    }

    #[test]
    fn test_find_lower_tile_y_negative_pos() {
        let pixels = create_uniform_image(64, 128, 0.5);
        let grid = make_grid(&pixels, 32);

        // Negative position should return 0
        assert_eq!(grid.find_lower_tile_y(-10.0), 0);
    }

    // --- Algorithm correctness tests ---
    // These verify the statistical algorithms produce mathematically correct results

    #[test]
    fn test_median_computation_correctness() {
        // Create image where we know exact median
        // Tile with values 1,2,3,4,5,6,7,8,9 should have median=5
        let width = 3;
        let height = 3;
        let data: Vec<f32> = (1..=9).map(|x| x as f32).collect();

        let pixels = Buffer2::new(width, height, data);
        let grid = make_grid(&pixels, 3);

        let stats = grid.get(0, 0);
        assert!(
            (stats.median - 5.0).abs() < 0.1,
            "Median of 1-9 should be 5, got {}",
            stats.median
        );
    }

    #[test]
    fn test_sigma_computation_correctness() {
        // For uniform data, sigma should be 0
        let pixels = create_uniform_image(64, 64, 100.0);
        let grid = make_grid(&pixels, 64);

        let stats = grid.get(0, 0);
        assert!(
            stats.sigma < 0.001,
            "Uniform data should have sigma ~0, got {}",
            stats.sigma
        );
    }

    #[test]
    fn test_mad_sigma_known_value() {
        // MAD-based sigma for known distribution
        // For values [0,1,2,3,4,5,6,7,8,9], median=4.5
        // Deviations from median: [4.5,3.5,2.5,1.5,0.5,0.5,1.5,2.5,3.5,4.5]
        // MAD = median of deviations = 2.5
        // sigma = MAD * 1.4826 ≈ 3.7
        let width = 10;
        let height = 1;
        let data: Vec<f32> = (0..10).map(|x| x as f32).collect();

        let pixels = Buffer2::new(width, height, data);

        // Use large tile to get all pixels
        let grid = make_grid(&pixels, 10);
        let stats = grid.get(0, 0);

        // Median should be 4.5
        assert!(
            (stats.median - 4.5).abs() < 0.1,
            "Median should be 4.5, got {}",
            stats.median
        );
        // Sigma should be ~3.7 (MAD * 1.4826)
        assert!(
            (stats.sigma - 3.7).abs() < 0.5,
            "Sigma should be ~3.7, got {}",
            stats.sigma
        );
    }

    #[test]
    fn test_3sigma_clipping_rejects_outliers() {
        // Background of 100 with a few extreme outliers
        // 3-sigma clipping should reject values > median + 3*sigma
        let width = 100;
        let height = 100;
        let mut data = vec![100.0; width * height];

        // Add 1% extreme outliers (100 pixels with value 10000)
        for i in 0..100 {
            data[i * 100] = 10000.0;
        }

        let pixels = Buffer2::new(width, height, data);
        let grid = make_grid(&pixels, 100);

        let stats = grid.get(0, 0);

        // After sigma clipping, median should still be ~100
        assert!(
            (stats.median - 100.0).abs() < 5.0,
            "Median should be ~100 after clipping outliers, got {}",
            stats.median
        );
    }

    #[test]
    fn test_median_filter_3x3_correctness() {
        // Create 5x5 grid of tiles where center tile has outlier value
        // After 3x3 median filter, center should match neighbors
        let width = 160; // 5 tiles of 32 pixels
        let height = 160;
        let mut data = vec![50.0; width * height];

        // Make center tile (tile 2,2) have value 200
        for y in 64..96 {
            for x in 64..96 {
                data[y * width + x] = 200.0;
            }
        }

        let pixels = Buffer2::new(width, height, data);
        let grid = make_grid(&pixels, 32);

        // Center tile should be filtered to ~50 (median of 8x50 + 1x200 = 50)
        let center = grid.get(2, 2);
        assert!(
            (center.median - 50.0).abs() < 10.0,
            "Center tile should be ~50 after median filter, got {}",
            center.median
        );
    }

    #[test]
    fn test_background_gradient_preserved() {
        // Linear gradient from 0 to 100 across image
        // Tile statistics should reflect local background level
        let width = 256;
        let height = 64;
        let data: Vec<f32> = (0..height)
            .flat_map(|_| (0..width).map(|x| x as f32 / width as f32 * 100.0))
            .collect();

        let pixels = Buffer2::new(width, height, data);
        let grid = make_grid(&pixels, 64);

        // Left tiles should have lower median than right tiles
        let left = grid.get(0, 0);
        let right = grid.get(3, 0);

        assert!(
            right.median > left.median + 30.0,
            "Right tile median {} should be > left {} + 30",
            right.median,
            left.median
        );
        assert!(
            left.median < 30.0,
            "Left tile median {} should be < 30",
            left.median
        );
        assert!(
            right.median > 70.0,
            "Right tile median {} should be > 70",
            right.median
        );
    }

    #[test]
    fn test_sparse_stars_rejected() {
        // Simulate astronomical image: mostly background (100) with sparse bright stars
        let width = 128;
        let height = 128;
        let mut data = vec![100.0; width * height];

        // Add 20 "stars" with brightness 500-1000 (random positions)
        let star_positions = [
            (10, 10),
            (50, 20),
            (100, 30),
            (30, 60),
            (80, 70),
            (120, 80),
            (15, 100),
            (60, 110),
            (90, 120),
            (110, 115),
            (25, 25),
            (75, 45),
            (45, 75),
            (95, 95),
            (5, 55),
            (55, 5),
            (105, 55),
            (55, 105),
            (35, 35),
            (85, 85),
        ];

        for (x, y) in star_positions {
            // Star with some spread
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    let nx = (x + dx).clamp(0, 127) as usize;
                    let ny = (y + dy).clamp(0, 127) as usize;
                    data[ny * width + nx] = 500.0 + (dx.abs() + dy.abs()) as f32 * -100.0;
                }
            }
        }

        let pixels = Buffer2::new(width, height, data);
        let grid = make_grid(&pixels, 64);

        // All tiles should have median close to background (100)
        for ty in 0..grid.tiles_y() {
            for tx in 0..grid.tiles_x() {
                let stats = grid.get(tx, ty);
                assert!(
                    (stats.median - 100.0).abs() < 20.0,
                    "Tile ({},{}) median {} should be ~100 (background)",
                    tx,
                    ty,
                    stats.median
                );
            }
        }
    }

    #[test]
    fn test_mask_excludes_sources_correctly() {
        // Background 50, sources at 200
        let width = 64;
        let height = 64;
        let mut data = vec![50.0; width * height];

        // Add bright source in top-left quadrant
        for y in 0..32 {
            for x in 0..32 {
                data[y * width + x] = 200.0;
            }
        }

        let pixels = Buffer2::new(width, height, data);

        // Mask the bright source
        let mut mask = BitBuffer2::new_filled(width, height, false);
        for y in 0..32 {
            for x in 0..32 {
                mask.set_xy(x, y, true);
            }
        }

        let grid = make_grid_with_mask(&pixels, 32, &mask, 100);

        // Top-left tile (0,0) should fallback to all pixels due to mask
        // Bottom-right tile (1,1) should have background value
        let br = grid.get(1, 1);
        assert!(
            (br.median - 50.0).abs() < 5.0,
            "Unmasked tile median {} should be ~50",
            br.median
        );
    }

    #[test]
    fn test_photutils_sextractor_comparison() {
        // Test case similar to photutils/SExtractor documentation examples
        // Background level 1000 with noise sigma ~10
        let width = 256;
        let height = 256;

        // Generate pseudo-random noise using deterministic pattern
        let data: Vec<f32> = (0..width * height)
            .map(|i| {
                let noise = ((i * 7919 + 104729) % 1000) as f32 / 100.0 - 5.0; // -5 to +5
                1000.0 + noise * 2.0 // background 1000, noise ~10
            })
            .collect();

        let pixels = Buffer2::new(width, height, data);
        let grid = make_grid(&pixels, 64);

        // Check all tiles have reasonable background estimate
        for ty in 0..grid.tiles_y() {
            for tx in 0..grid.tiles_x() {
                let stats = grid.get(tx, ty);
                // Background should be ~1000 ± 5
                assert!(
                    (stats.median - 1000.0).abs() < 10.0,
                    "Tile ({},{}) median {} should be ~1000",
                    tx,
                    ty,
                    stats.median
                );
                // Sigma should be reasonable (not zero, not huge)
                assert!(
                    stats.sigma > 1.0 && stats.sigma < 30.0,
                    "Tile ({},{}) sigma {} should be reasonable",
                    tx,
                    ty,
                    stats.sigma
                );
            }
        }
    }
}
