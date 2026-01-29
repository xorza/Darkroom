//! Local normalization for multi-session astrophotography stacking.
//!
//! This module implements PixInsight-style local normalization to match
//! illumination differences across frames by adjusting brightness locally
//! rather than globally. This handles:
//! - Vignetting (darker corners, brighter center)
//! - Sky gradients (light pollution, moon, twilight)
//! - Session-to-session brightness variations
//!
//! # Algorithm
//!
//! 1. Divide image into tiles (default: 128x128 pixels)
//! 2. Compute sigma-clipped median and scale (MAD) for each tile
//! 3. Compare target frame tiles to reference frame tiles
//! 4. Compute per-tile offset and scale correction factors
//! 5. Bilinearly interpolate between tile centers for smooth correction
//! 6. Apply: `pixel_corrected = (pixel - target_median) * scale + ref_median`

use rayon::prelude::*;

use crate::common::Buffer2;
use crate::star_detection::constants::sigma_clipped_median_mad;

/// Number of rows to process per parallel chunk for interpolation.
const ROWS_PER_CHUNK: usize = 8;

/// Normalization method for aligning frame statistics before stacking.
///
/// Controls how frame statistics are matched before pixel rejection and integration.
#[derive(Debug, Clone, PartialEq, Default)]
#[allow(dead_code)] // Public API - integration with stacking pipeline pending
pub enum NormalizationMethod {
    /// No normalization - use raw pixel values.
    None,
    /// Global normalization - match overall median and scale.
    /// Simple and fast, works well for single-session data.
    #[default]
    Global,
    /// Local normalization - tile-based matching.
    /// Best for multi-session data or frames with varying gradients.
    Local(LocalNormalizationConfig),
}

/// Configuration for local normalization.
///
/// Local normalization divides the image into tiles and computes correction
/// factors per-tile, then interpolates smoothly between tile centers.
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)] // Public API - integration with stacking pipeline pending
pub struct LocalNormalizationConfig {
    /// Tile size in pixels. Larger tiles are more robust to noise but
    /// less accurate for steep gradients.
    /// Default: 128, Range: 64-256
    pub tile_size: usize,
    /// Sigma threshold for clipping outliers (stars) when computing tile statistics.
    /// Default: 3.0
    pub clip_sigma: f32,
    /// Number of sigma-clipping iterations.
    /// Default: 3
    pub clip_iterations: usize,
}

impl Default for LocalNormalizationConfig {
    fn default() -> Self {
        Self {
            tile_size: 128,
            clip_sigma: 3.0,
            clip_iterations: 3,
        }
    }
}

#[allow(dead_code)] // Public API - integration with stacking pipeline pending
impl LocalNormalizationConfig {
    /// Create a new configuration with the specified tile size.
    ///
    /// # Panics
    ///
    /// Panics if tile_size is not in the range 64-256.
    pub fn new(tile_size: usize) -> Self {
        assert!(
            (64..=256).contains(&tile_size),
            "Tile size must be between 64 and 256, got {}",
            tile_size
        );
        Self {
            tile_size,
            ..Default::default()
        }
    }

    /// Set custom clipping parameters.
    pub fn with_clipping(mut self, sigma: f32, iterations: usize) -> Self {
        assert!(sigma > 0.0, "Clip sigma must be positive");
        assert!(iterations > 0, "Clip iterations must be at least 1");
        self.clip_sigma = sigma;
        self.clip_iterations = iterations;
        self
    }

    /// Create configuration optimized for fine gradients (smaller tiles).
    pub fn fine() -> Self {
        Self {
            tile_size: 64,
            ..Default::default()
        }
    }

    /// Create configuration optimized for stability (larger tiles).
    pub fn coarse() -> Self {
        Self {
            tile_size: 256,
            ..Default::default()
        }
    }
}

/// Per-tile normalization statistics computed from a frame.
#[derive(Debug, Clone)]
#[allow(dead_code)] // Public API - integration with stacking pipeline pending
pub struct TileNormalizationStats {
    /// Per-tile median values.
    pub medians: Vec<f32>,
    /// Per-tile scale (sigma from MAD) values.
    pub scales: Vec<f32>,
    /// Number of tiles in X direction.
    pub tiles_x: usize,
    /// Number of tiles in Y direction.
    pub tiles_y: usize,
    /// Tile size used for computation.
    pub tile_size: usize,
    /// Image width.
    pub width: usize,
    /// Image height.
    pub height: usize,
}

/// Local normalization map for correcting a target frame to match a reference.
///
/// Contains per-tile offset and scale factors that can be interpolated
/// to correct individual pixels.
#[derive(Debug, Clone)]
#[allow(dead_code)] // Public API - integration with stacking pipeline pending
pub struct LocalNormalizationMap {
    /// Per-tile offset (additive correction after scaling).
    /// Applied as: corrected = (pixel - target_median) * scale + ref_median
    /// The offset here stores ref_median for each tile.
    offsets: Vec<f32>,
    /// Per-tile scale (multiplicative correction).
    /// scale = ref_scale / target_scale (or 1.0 if target_scale is near zero)
    scales: Vec<f32>,
    /// Per-tile target median for subtraction before scaling.
    target_medians: Vec<f32>,
    /// Tile centers X coordinates for interpolation.
    centers_x: Vec<f32>,
    /// Tile centers Y coordinates for interpolation.
    centers_y: Vec<f32>,
    /// Number of tiles in X direction.
    tiles_x: usize,
    /// Number of tiles in Y direction.
    tiles_y: usize,
    /// Image width.
    width: usize,
    /// Image height.
    height: usize,
}

// ============================================================================
// Tile Statistics Computation
// ============================================================================

impl TileNormalizationStats {
    /// Compute tile-based statistics for a frame.
    ///
    /// Divides the image into tiles and computes sigma-clipped median and scale
    /// for each tile. Stars and other outliers are rejected during statistics
    /// computation to get accurate background estimates.
    ///
    /// # Arguments
    /// * `pixels` - Grayscale image buffer
    /// * `config` - Local normalization configuration
    ///
    /// # Panics
    ///
    /// Panics if the image is smaller than the tile size.
    pub fn compute(pixels: &Buffer2<f32>, config: &LocalNormalizationConfig) -> Self {
        let width = pixels.width();
        let height = pixels.height();
        let tile_size = config.tile_size;
        assert!(
            width >= tile_size && height >= tile_size,
            "Image ({width}x{height}) must be at least tile_size ({tile_size}x{tile_size})"
        );

        let tiles_x = width.div_ceil(tile_size);
        let tiles_y = height.div_ceil(tile_size);
        let max_tile_pixels = tile_size * tile_size;

        // Compute statistics for each tile in parallel
        let tile_results: Vec<(f32, f32)> = (0..tiles_y * tiles_x)
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

                    // Collect tile pixels
                    values_buf.clear();
                    for y in y_start..y_end {
                        let row_start = y * width + x_start;
                        values_buf
                            .extend_from_slice(&pixels[row_start..row_start + (x_end - x_start)]);
                    }

                    // Compute sigma-clipped statistics
                    sigma_clipped_median_mad(
                        values_buf,
                        deviations_buf,
                        config.clip_sigma,
                        config.clip_iterations,
                    )
                },
            )
            .collect();

        // Unzip results
        let (medians, scales): (Vec<f32>, Vec<f32>) = tile_results.into_iter().unzip();

        Self {
            medians,
            scales,
            tiles_x,
            tiles_y,
            tile_size,
            width,
            height,
        }
    }
}

// ============================================================================
// Normalization Map Computation
// ============================================================================

impl LocalNormalizationMap {
    /// Compute a normalization map from reference and target tile statistics.
    ///
    /// For each tile, computes:
    /// - offset = ref_median (stored for the correction formula)
    /// - scale = ref_scale / target_scale (clamped to avoid division by near-zero)
    /// - target_median (for subtraction before scaling)
    ///
    /// The correction formula is: `corrected = (pixel - target_median) * scale + ref_median`
    ///
    /// # Arguments
    /// * `reference` - Tile statistics from the reference frame
    /// * `target` - Tile statistics from the frame to be normalized
    ///
    /// # Panics
    ///
    /// Panics if the reference and target have different tile configurations.
    pub fn compute(reference: &TileNormalizationStats, target: &TileNormalizationStats) -> Self {
        assert_eq!(
            reference.tiles_x, target.tiles_x,
            "Reference and target must have same tile count in X"
        );
        assert_eq!(
            reference.tiles_y, target.tiles_y,
            "Reference and target must have same tile count in Y"
        );
        assert_eq!(
            reference.tile_size, target.tile_size,
            "Reference and target must have same tile size"
        );
        assert_eq!(
            reference.width, target.width,
            "Reference and target must have same width"
        );
        assert_eq!(
            reference.height, target.height,
            "Reference and target must have same height"
        );

        let tile_size = reference.tile_size;
        let tiles_x = reference.tiles_x;
        let tiles_y = reference.tiles_y;
        let width = reference.width;
        let height = reference.height;
        let num_tiles = tiles_x * tiles_y;

        // Minimum scale value to avoid division by near-zero
        const MIN_SCALE: f32 = 1e-6;

        // Compute correction factors
        let mut offsets = Vec::with_capacity(num_tiles);
        let mut scales = Vec::with_capacity(num_tiles);
        let mut target_medians = Vec::with_capacity(num_tiles);

        for i in 0..num_tiles {
            let ref_median = reference.medians[i];
            let ref_scale = reference.scales[i];
            let tgt_median = target.medians[i];
            let tgt_scale = target.scales[i];

            // Scale factor: ref_scale / target_scale
            // Clamp target_scale to avoid division by near-zero
            let scale = if tgt_scale.abs() > MIN_SCALE {
                ref_scale / tgt_scale
            } else {
                1.0 // No scale correction if target scale is near zero
            };

            offsets.push(ref_median);
            scales.push(scale);
            target_medians.push(tgt_median);
        }

        // Compute tile centers for interpolation
        let centers_x: Vec<f32> = (0..tiles_x)
            .map(|tx| {
                let x_start = tx * tile_size;
                let x_end = (x_start + tile_size).min(width);
                (x_start + x_end) as f32 * 0.5
            })
            .collect();

        let centers_y: Vec<f32> = (0..tiles_y)
            .map(|ty| {
                let y_start = ty * tile_size;
                let y_end = (y_start + tile_size).min(height);
                (y_start + y_end) as f32 * 0.5
            })
            .collect();

        Self {
            offsets,
            scales,
            target_medians,
            centers_x,
            centers_y,
            tiles_x,
            tiles_y,
            width,
            height,
        }
    }

    /// Apply local normalization to an image in-place.
    ///
    /// Uses bilinear interpolation between tile centers for smooth correction.
    /// The correction formula is: `corrected = (pixel - target_median) * scale + ref_median`
    ///
    /// # Arguments
    /// * `pixels` - Image to normalize (modified in-place)
    ///
    /// # Panics
    ///
    /// Panics if the pixel buffer dimensions don't match the map dimensions.
    pub fn apply(&self, pixels: &mut Buffer2<f32>) {
        assert_eq!(
            pixels.width(),
            self.width,
            "Width mismatch: expected {}, got {}",
            self.width,
            pixels.width()
        );
        assert_eq!(
            pixels.height(),
            self.height,
            "Height mismatch: expected {}, got {}",
            self.height,
            pixels.height()
        );

        let width = self.width;

        // Process in parallel chunks of rows
        pixels
            .par_chunks_mut(width * ROWS_PER_CHUNK)
            .enumerate()
            .for_each(|(chunk_idx, chunk)| {
                let y_start = chunk_idx * ROWS_PER_CHUNK;
                let rows_in_chunk = chunk.len() / width;

                for local_y in 0..rows_in_chunk {
                    let y = y_start + local_y;
                    let row_offset = local_y * width;
                    let row = &mut chunk[row_offset..row_offset + width];

                    self.apply_row(row, y);
                }
            });
    }

    /// Apply local normalization to a single row.
    fn apply_row(&self, row: &mut [f32], y: usize) {
        let fy = y as f32;
        let width = row.len();

        // Compute Y tile indices and weight once for the entire row
        let ty0 = find_lower_tile(fy, &self.centers_y);
        let ty1 = (ty0 + 1).min(self.tiles_y - 1);

        let wy = if ty1 != ty0 {
            ((fy - self.centers_y[ty0]) / (self.centers_y[ty1] - self.centers_y[ty0]))
                .clamp(0.0, 1.0)
        } else {
            0.0
        };
        let wy_inv = 1.0 - wy;

        // Process row in segments between tile center X boundaries
        let mut x = 0usize;

        for tx0 in 0..self.tiles_x {
            let tx1 = (tx0 + 1).min(self.tiles_x - 1);

            // Segment runs from current x to next tile center (or end of row)
            let segment_end = if tx0 + 1 < self.tiles_x {
                (self.centers_x[tx0 + 1].floor() as usize).min(width)
            } else {
                width
            };

            if segment_end <= x {
                continue;
            }

            // Get the four corner tiles for bilinear interpolation
            let idx00 = ty0 * self.tiles_x + tx0;
            let idx10 = ty0 * self.tiles_x + tx1;
            let idx01 = ty1 * self.tiles_x + tx0;
            let idx11 = ty1 * self.tiles_x + tx1;

            // Interpolate correction parameters
            let left_offset = wy_inv * self.offsets[idx00] + wy * self.offsets[idx01];
            let right_offset = wy_inv * self.offsets[idx10] + wy * self.offsets[idx11];
            let left_scale = wy_inv * self.scales[idx00] + wy * self.scales[idx01];
            let right_scale = wy_inv * self.scales[idx10] + wy * self.scales[idx11];
            let left_tgt_med =
                wy_inv * self.target_medians[idx00] + wy * self.target_medians[idx01];
            let right_tgt_med =
                wy_inv * self.target_medians[idx10] + wy * self.target_medians[idx11];

            let segment = &mut row[x..segment_end];

            if tx1 != tx0 {
                // Need X interpolation
                let inv_dx = 1.0 / (self.centers_x[tx1] - self.centers_x[tx0]);
                let x_offset = self.centers_x[tx0];
                let wx_start = (x as f32 - x_offset) * inv_dx;
                let wx_step = inv_dx;

                apply_segment_interpolated(
                    segment,
                    left_offset,
                    right_offset,
                    left_scale,
                    right_scale,
                    left_tgt_med,
                    right_tgt_med,
                    wx_start,
                    wx_step,
                );
            } else {
                // Constant correction (single tile column)
                apply_segment_constant(segment, left_offset, left_scale, left_tgt_med);
            }

            x = segment_end;
            if x >= width {
                break;
            }
        }
    }

    /// Apply local normalization, returning a new normalized image.
    ///
    /// This is a convenience method that clones the input and applies normalization.
    pub fn apply_to_new(&self, pixels: &Buffer2<f32>) -> Buffer2<f32> {
        let mut result = pixels.clone();
        self.apply(&mut result);
        result
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

/// Apply constant correction to a segment.
#[inline]
fn apply_segment_constant(segment: &mut [f32], offset: f32, scale: f32, target_median: f32) {
    for px in segment.iter_mut() {
        *px = (*px - target_median) * scale + offset;
    }
}

/// Apply interpolated correction to a segment with SIMD acceleration.
///
/// Uses the same interpolation approach as background estimation but with
/// custom correction formula: corrected = (pixel - target_median) * scale + offset
#[allow(clippy::too_many_arguments)]
fn apply_segment_interpolated(
    segment: &mut [f32],
    left_offset: f32,
    right_offset: f32,
    left_scale: f32,
    right_scale: f32,
    left_tgt_med: f32,
    right_tgt_med: f32,
    wx_start: f32,
    wx_step: f32,
) {
    // We need to interpolate offset, scale, and target_median per pixel
    // then apply: corrected = (pixel - target_median) * scale + offset
    //
    // For SIMD efficiency, we compute interpolated values and apply in one pass

    let len = segment.len();
    let delta_offset = right_offset - left_offset;
    let delta_scale = right_scale - left_scale;
    let delta_tgt_med = right_tgt_med - left_tgt_med;

    for (i, px) in segment.iter_mut().enumerate() {
        let wx = (wx_start + i as f32 * wx_step).clamp(0.0, 1.0);
        let offset = left_offset + wx * delta_offset;
        let scale = left_scale + wx * delta_scale;
        let target_median = left_tgt_med + wx * delta_tgt_med;

        *px = (*px - target_median) * scale + offset;
    }

    // Note: We could add SIMD optimization here similar to interpolate_segment_simd
    // but the per-pixel read-modify-write pattern is different from pure interpolation.
    // The current implementation is still quite fast due to simple arithmetic.
    let _ = len; // Suppress unused warning
}

// ============================================================================
// Convenience Functions
// ============================================================================

/// Compute local normalization statistics and map for a target frame.
///
/// This is a convenience function that combines `TileNormalizationStats::compute`
/// and `LocalNormalizationMap::compute`.
///
/// # Arguments
/// * `reference_pixels` - Reference frame pixel buffer
/// * `target_pixels` - Target frame pixel buffer (to be normalized)
/// * `config` - Local normalization configuration
///
/// # Returns
/// A `LocalNormalizationMap` that can be applied to normalize the target frame.
#[allow(dead_code)] // Public API - integration with stacking pipeline pending
pub fn compute_normalization_map(
    reference_pixels: &Buffer2<f32>,
    target_pixels: &Buffer2<f32>,
    config: &LocalNormalizationConfig,
) -> LocalNormalizationMap {
    let ref_stats = TileNormalizationStats::compute(reference_pixels, config);
    let target_stats = TileNormalizationStats::compute(target_pixels, config);
    LocalNormalizationMap::compute(&ref_stats, &target_stats)
}

#[allow(dead_code)] // Public API - integration with stacking pipeline pending
#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::synthetic::patterns;

    /// Apply local normalization to a frame, returning the normalized result.
    ///
    /// This is a convenience function that computes the normalization map and applies it.
    ///
    /// # Arguments
    /// * `reference_pixels` - Reference frame pixel buffer
    /// * `target_pixels` - Target frame pixel buffer (to be normalized)
    /// * `config` - Local normalization configuration
    ///
    /// # Returns
    /// Normalized version of the target frame.
    pub fn normalize_frame(
        reference_pixels: &Buffer2<f32>,
        target_pixels: &Buffer2<f32>,
        config: &LocalNormalizationConfig,
    ) -> Buffer2<f32> {
        let map = compute_normalization_map(reference_pixels, target_pixels, config);
        map.apply_to_new(target_pixels)
    }

    // ========== Config Tests ==========

    #[test]
    fn test_normalization_method_default() {
        let method = NormalizationMethod::default();
        assert!(matches!(method, NormalizationMethod::Global));
    }

    #[test]
    fn test_local_normalization_config_default() {
        let config = LocalNormalizationConfig::default();
        assert_eq!(config.tile_size, 128);
        assert!((config.clip_sigma - 3.0).abs() < f32::EPSILON);
        assert_eq!(config.clip_iterations, 3);
    }

    #[test]
    fn test_local_normalization_config_new() {
        let config = LocalNormalizationConfig::new(64);
        assert_eq!(config.tile_size, 64);
    }

    #[test]
    #[should_panic(expected = "Tile size must be between 64 and 256")]
    fn test_local_normalization_config_invalid_tile_size_small() {
        LocalNormalizationConfig::new(32);
    }

    #[test]
    #[should_panic(expected = "Tile size must be between 64 and 256")]
    fn test_local_normalization_config_invalid_tile_size_large() {
        LocalNormalizationConfig::new(512);
    }

    #[test]
    fn test_local_normalization_config_with_clipping() {
        let config = LocalNormalizationConfig::default().with_clipping(2.5, 5);
        assert!((config.clip_sigma - 2.5).abs() < f32::EPSILON);
        assert_eq!(config.clip_iterations, 5);
    }

    #[test]
    #[should_panic(expected = "Clip sigma must be positive")]
    fn test_local_normalization_config_invalid_sigma() {
        LocalNormalizationConfig::default().with_clipping(0.0, 3);
    }

    #[test]
    #[should_panic(expected = "Clip iterations must be at least 1")]
    fn test_local_normalization_config_invalid_iterations() {
        LocalNormalizationConfig::default().with_clipping(3.0, 0);
    }

    #[test]
    fn test_local_normalization_config_fine() {
        let config = LocalNormalizationConfig::fine();
        assert_eq!(config.tile_size, 64);
    }

    #[test]
    fn test_local_normalization_config_coarse() {
        let config = LocalNormalizationConfig::coarse();
        assert_eq!(config.tile_size, 256);
    }

    #[test]
    fn test_normalization_method_none() {
        let method = NormalizationMethod::None;
        assert!(matches!(method, NormalizationMethod::None));
    }

    #[test]
    fn test_normalization_method_local() {
        let config = LocalNormalizationConfig::default();
        let method = NormalizationMethod::Local(config.clone());
        assert!(matches!(method, NormalizationMethod::Local(_)));

        if let NormalizationMethod::Local(c) = method {
            assert_eq!(c.tile_size, config.tile_size);
        }
    }

    // ========== Tile Statistics Tests ==========

    #[test]
    fn test_tile_stats_uniform_image() {
        let pixels = patterns::uniform(256, 256, 100.0);
        let config = LocalNormalizationConfig::new(64);

        let stats = TileNormalizationStats::compute(&pixels, &config);

        // 256 / 64 = 4 tiles in each direction
        assert_eq!(stats.tiles_x, 4);
        assert_eq!(stats.tiles_y, 4);
        assert_eq!(stats.medians.len(), 16);
        assert_eq!(stats.scales.len(), 16);

        // All tiles should have median ≈ 100 and scale ≈ 0 (uniform)
        for &median in &stats.medians {
            assert!(
                (median - 100.0).abs() < 1e-4,
                "Median should be 100, got {}",
                median
            );
        }
        for &scale in &stats.scales {
            assert!(scale.abs() < 1e-4, "Scale should be 0, got {}", scale);
        }
    }

    #[test]
    fn test_tile_stats_horizontal_gradient() {
        let pixels = patterns::horizontal_gradient(256, 128, 50.0, 150.0);
        let config = LocalNormalizationConfig::new(64);

        let stats = TileNormalizationStats::compute(&pixels, &config);

        // 256 / 64 = 4 tiles in X, 128 / 64 = 2 tiles in Y
        assert_eq!(stats.tiles_x, 4);
        assert_eq!(stats.tiles_y, 2);

        // Medians should increase from left to right
        // First column (tiles 0, 4): center at x=32 -> value ≈ 50 + (32/255)*100 ≈ 62.5
        // Last column (tiles 3, 7): center at x=224 -> value ≈ 50 + (224/255)*100 ≈ 137.8
        let left_median = (stats.medians[0] + stats.medians[4]) / 2.0;
        let right_median = (stats.medians[3] + stats.medians[7]) / 2.0;

        assert!(
            right_median > left_median,
            "Right median ({}) should be greater than left ({}) in horizontal gradient",
            right_median,
            left_median
        );
    }

    #[test]
    fn test_tile_stats_vertical_gradient() {
        let pixels = patterns::vertical_gradient(128, 256, 0.0, 200.0);
        let config = LocalNormalizationConfig::new(64);

        let stats = TileNormalizationStats::compute(&pixels, &config);

        // 128 / 64 = 2 tiles in X, 256 / 64 = 4 tiles in Y
        assert_eq!(stats.tiles_x, 2);
        assert_eq!(stats.tiles_y, 4);

        // Medians should increase from top to bottom
        let top_median = (stats.medians[0] + stats.medians[1]) / 2.0;
        let bottom_median = (stats.medians[6] + stats.medians[7]) / 2.0;

        assert!(
            bottom_median > top_median,
            "Bottom median ({}) should be greater than top ({}) in vertical gradient",
            bottom_median,
            top_median
        );
    }

    // ========== Normalization Map Tests ==========

    #[test]
    fn test_normalization_map_identical_frames() {
        let pixels = patterns::uniform(256, 256, 100.0);
        let config = LocalNormalizationConfig::new(64);

        let ref_stats = TileNormalizationStats::compute(&pixels, &config);
        let target_stats = TileNormalizationStats::compute(&pixels, &config);
        let map = LocalNormalizationMap::compute(&ref_stats, &target_stats);

        // For identical frames, scales should be 1.0 (or near 1.0)
        // and offsets should equal medians
        for &scale in &map.scales {
            // With zero variance, scale defaults to 1.0
            assert!(
                (scale - 1.0).abs() < 1e-4,
                "Scale should be 1.0 for identical frames, got {}",
                scale
            );
        }
    }

    #[test]
    fn test_normalization_map_offset_correction() {
        let reference = patterns::uniform(256, 128, 100.0);
        let target = patterns::uniform(256, 128, 80.0); // 20 units darker
        let config = LocalNormalizationConfig::new(64);

        let ref_stats = TileNormalizationStats::compute(&reference, &config);
        let target_stats = TileNormalizationStats::compute(&target, &config);
        let map = LocalNormalizationMap::compute(&ref_stats, &target_stats);

        // Apply normalization - should bring target up to reference level
        let normalized = map.apply_to_new(&target);

        // Check that normalized values are close to reference
        let avg_normalized: f32 = normalized.iter().sum::<f32>() / normalized.len() as f32;
        assert!(
            (avg_normalized - 100.0).abs() < 1.0,
            "Normalized average should be ~100, got {}",
            avg_normalized
        );
    }

    #[test]
    fn test_normalization_map_gradient_correction() {
        let reference = patterns::uniform(256, 128, 100.0);
        let target = patterns::horizontal_gradient(256, 128, 80.0, 120.0); // Gradient
        let config = LocalNormalizationConfig::new(64);

        let ref_stats = TileNormalizationStats::compute(&reference, &config);
        let target_stats = TileNormalizationStats::compute(&target, &config);
        let map = LocalNormalizationMap::compute(&ref_stats, &target_stats);

        // Apply normalization
        let normalized = map.apply_to_new(&target);

        // The gradient should be flattened out
        // Compute variance before and after
        let target_variance = compute_variance(&target);
        let normalized_variance = compute_variance(&normalized);

        assert!(
            normalized_variance < target_variance,
            "Normalized variance ({}) should be less than target variance ({})",
            normalized_variance,
            target_variance
        );
    }

    fn compute_variance(pixels: &[f32]) -> f32 {
        let n = pixels.len() as f32;
        let mean = pixels.iter().sum::<f32>() / n;
        pixels.iter().map(|&x| (x - mean).powi(2)).sum::<f32>() / n
    }

    // ========== Apply Normalization Tests ==========

    #[test]
    fn test_apply_in_place() {
        let reference = patterns::uniform(256, 128, 100.0);
        let target = patterns::uniform(256, 128, 50.0);
        let config = LocalNormalizationConfig::new(64);

        let map = compute_normalization_map(&reference, &target, &config);

        let mut target_mut = target.clone();
        map.apply(&mut target_mut);

        // Should be the same as apply_to_new
        let normalized = map.apply_to_new(&target);

        for (a, b) in target_mut.iter().zip(normalized.iter()) {
            assert!(
                (a - b).abs() < 1e-6,
                "In-place and new apply should give same result"
            );
        }
    }

    #[test]
    fn test_normalize_frame_convenience() {
        let reference = patterns::uniform(256, 128, 100.0);
        let target = patterns::uniform(256, 128, 75.0);
        let config = LocalNormalizationConfig::new(64);

        let normalized = normalize_frame(&reference, &target, &config);

        let avg: f32 = normalized.iter().sum::<f32>() / normalized.len() as f32;
        assert!(
            (avg - 100.0).abs() < 1.0,
            "Normalized should match reference level, got {}",
            avg
        );
    }

    // ========== Edge Cases ==========

    #[test]
    fn test_single_tile_image() {
        let reference = patterns::uniform(64, 64, 100.0);
        let target = patterns::uniform(64, 64, 50.0);
        let config = LocalNormalizationConfig::new(64);

        let stats = TileNormalizationStats::compute(&reference, &config);
        assert_eq!(stats.tiles_x, 1);
        assert_eq!(stats.tiles_y, 1);

        let normalized = normalize_frame(&reference, &target, &config);
        let avg: f32 = normalized.iter().sum::<f32>() / normalized.len() as f32;
        assert!(
            (avg - 100.0).abs() < 1.0,
            "Single tile normalization should work"
        );
    }

    #[test]
    fn test_non_multiple_tile_size() {
        // Image dimensions not evenly divisible by tile size
        // 300 / 64 = 4 full tiles + 44 partial
        // 150 / 64 = 2 full tiles + 22 partial
        let reference = patterns::uniform(300, 150, 100.0);
        let target = patterns::uniform(300, 150, 80.0);
        let config = LocalNormalizationConfig::new(64);

        let stats = TileNormalizationStats::compute(&reference, &config);
        assert_eq!(stats.tiles_x, 5); // ceil(300/64) = 5
        assert_eq!(stats.tiles_y, 3); // ceil(150/64) = 3

        let normalized = normalize_frame(&reference, &target, &config);
        let avg: f32 = normalized.iter().sum::<f32>() / normalized.len() as f32;
        assert!(
            (avg - 100.0).abs() < 1.0,
            "Non-multiple size should still work correctly"
        );
    }

    #[test]
    #[should_panic(expected = "must be at least tile_size")]
    fn test_image_too_small_for_tile_size() {
        let pixels = patterns::uniform(32, 32, 100.0);
        let config = LocalNormalizationConfig::new(64);

        TileNormalizationStats::compute(&pixels, &config);
    }

    // ========== Helper Function Tests ==========

    #[test]
    fn test_find_lower_tile() {
        let centers = vec![32.0, 96.0, 160.0, 224.0];

        assert_eq!(find_lower_tile(0.0, &centers), 0);
        assert_eq!(find_lower_tile(32.0, &centers), 0);
        assert_eq!(find_lower_tile(50.0, &centers), 0);
        assert_eq!(find_lower_tile(96.0, &centers), 1);
        assert_eq!(find_lower_tile(100.0, &centers), 1);
        assert_eq!(find_lower_tile(200.0, &centers), 2);
        assert_eq!(find_lower_tile(300.0, &centers), 3);
    }

    #[test]
    fn test_apply_segment_constant() {
        let mut segment = vec![100.0, 100.0, 100.0, 100.0];
        let offset = 50.0; // ref_median
        let scale = 2.0;
        let target_median = 80.0;

        apply_segment_constant(&mut segment, offset, scale, target_median);

        // corrected = (100 - 80) * 2 + 50 = 20 * 2 + 50 = 90
        for &v in &segment {
            assert!((v - 90.0).abs() < 1e-6, "Expected 90, got {}", v);
        }
    }
}
