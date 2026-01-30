//! Scalar implementation of X-Trans bilinear demosaicing.
//!
//! This is a basic bilinear interpolation for X-Trans sensors.
//! It's simpler and faster than advanced algorithms like Markesteijn,
//! but produces lower quality results (may show artifacts).
//!
//! # Optimizations
//!
//! ## Pre-computed Neighbor Offset Tables
//!
//! The X-Trans CFA pattern is a 6x6 repeating grid where each pixel only samples
//! one color (R, G, or B). To reconstruct full RGB at each pixel, we need to
//! interpolate the missing channels from nearby pixels of the target color.
//!
//! **Naive approach (slow):** For each output pixel, scan the 5x5 neighborhood
//! and check pattern.color_at(y+dy, x+dx) for each (dy, dx). This involves:
//! - 25 pattern lookups per channel × 2 missing channels = 50 lookups/pixel
//! - Each lookup involves modulo operations on y and x
//!
//! **Optimized approach (OffsetList):** Pre-compute which (dy, dx) offsets contain
//! each color for each of the 36 pattern positions. At runtime, we just iterate
//! the pre-computed list without any pattern lookups or modulo operations.
//!
//! ## Pre-computed Linear Offsets (LinearOffsetList)
//!
//! Taking optimization further: in the hot loop, converting (dy, dx) to a linear
//! index requires `base_idx + dy * stride + dx`. The multiply by stride is expensive.
//!
//! **Solution:** Pre-compute `dy * stride + dx` as a single `isize` offset for each
//! neighbor, creating `LinearNeighborLookup`. This requires knowing the image stride
//! at construction time, but eliminates the multiply from the inner loop entirely.
//!
//! The linear offset can then be used with simple pointer arithmetic:
//! `data_ptr.offset(linear_offset)` instead of `data[y * stride + x]`.
//!
//! ## Other Optimizations
//! - Separate fast path for interior pixels (no bounds checking)
//! - Row base index pre-computation
//! - Parallel processing via rayon for large images
//! - SIMD acceleration on x86_64 (SSE4.1) and aarch64 (NEON)

use rayon::prelude::*;

use super::{XTransImage, XTransPattern};

/// Minimum image size to use parallel processing (avoids overhead for small images).
const MIN_PARALLEL_SIZE: usize = 128;

/// Search radius for interpolation (5x5 neighborhood).
const SEARCH_RADIUS: usize = 2;

/// Maximum number of neighbors in a 5x5 window.
const MAX_NEIGHBORS: usize = 25;

/// Fixed-size storage for neighbor offsets at one pattern position.
#[derive(Debug, Clone, Copy)]
pub(super) struct OffsetList {
    offsets: [(i32, i32); MAX_NEIGHBORS],
    len: u8,
    /// Pre-computed reciprocal of len for fast averaging (1.0 / len).
    inv_len: f32,
}

impl OffsetList {
    const fn new() -> Self {
        Self {
            offsets: [(0, 0); MAX_NEIGHBORS],
            len: 0,
            inv_len: 0.0,
        }
    }

    #[inline(always)]
    fn push(&mut self, offset: (i32, i32)) {
        debug_assert!((self.len as usize) < MAX_NEIGHBORS);
        self.offsets[self.len as usize] = offset;
        self.len += 1;
    }

    /// Finalize the list by computing the reciprocal of len.
    #[inline(always)]
    fn finalize(&mut self) {
        if self.len > 0 {
            self.inv_len = 1.0 / self.len as f32;
        }
    }

    #[inline(always)]
    pub(super) fn as_slice(&self) -> &[(i32, i32)] {
        &self.offsets[..self.len as usize]
    }

    #[inline(always)]
    pub(super) fn inv_len(&self) -> f32 {
        self.inv_len
    }
}

/// Pre-computed neighbor offsets for each color within the 5x5 search window.
/// For each position in the 6x6 pattern, stores the relative offsets (dy, dx)
/// where each color (R=0, G=1, B=2) can be found.
#[derive(Debug)]
pub(super) struct NeighborLookup {
    /// Offsets for each color at each pattern position.
    offsets: [[OffsetList; 6]; 6],
}

impl NeighborLookup {
    pub(super) fn new(pattern: &XTransPattern, color: u8) -> Self {
        let mut offsets = [[OffsetList::new(); 6]; 6];

        // For each position in the 6x6 pattern
        for (py, row) in offsets.iter_mut().enumerate() {
            for (px, cell) in row.iter_mut().enumerate() {
                // Find all neighbors of the target color in 5x5 window
                for dy in -(SEARCH_RADIUS as i32)..=(SEARCH_RADIUS as i32) {
                    for dx in -(SEARCH_RADIUS as i32)..=(SEARCH_RADIUS as i32) {
                        // Neighbor position in pattern (with wrapping for lookup)
                        let ny = (py as i32 + dy).rem_euclid(6) as usize;
                        let nx = (px as i32 + dx).rem_euclid(6) as usize;

                        if pattern.pattern[ny][nx] == color {
                            cell.push((dy, dx));
                        }
                    }
                }
                cell.finalize();
            }
        }

        Self { offsets }
    }

    #[inline(always)]
    pub(super) fn get(&self, pattern_y: usize, pattern_x: usize) -> &OffsetList {
        &self.offsets[pattern_y % 6][pattern_x % 6]
    }
}

/// Fixed-size storage for pre-computed linear offsets at one pattern position.
#[derive(Debug, Clone, Copy)]
pub(super) struct LinearOffsetList {
    offsets: [isize; MAX_NEIGHBORS],
    len: u8,
    /// Pre-computed reciprocal of len for fast averaging (1.0 / len).
    inv_len: f32,
}

impl LinearOffsetList {
    const fn new() -> Self {
        Self {
            offsets: [0; MAX_NEIGHBORS],
            len: 0,
            inv_len: 0.0,
        }
    }

    #[inline(always)]
    fn push(&mut self, offset: isize) {
        debug_assert!((self.len as usize) < MAX_NEIGHBORS);
        self.offsets[self.len as usize] = offset;
        self.len += 1;
    }

    #[inline(always)]
    fn finalize(&mut self) {
        if self.len > 0 {
            self.inv_len = 1.0 / self.len as f32;
        }
    }

    #[inline(always)]
    pub(super) fn as_slice(&self) -> &[isize] {
        &self.offsets[..self.len as usize]
    }

    #[inline(always)]
    pub(super) fn inv_len(&self) -> f32 {
        self.inv_len
    }
}

/// Pre-computed linear offsets (dy * stride + dx) for fast neighbor access.
/// These are computed once per image based on the actual stride.
#[derive(Debug)]
pub(super) struct LinearNeighborLookup {
    /// Linear offsets for each pattern position.
    offsets: [[LinearOffsetList; 6]; 6],
}

impl LinearNeighborLookup {
    pub(super) fn new(pattern: &XTransPattern, color: u8, stride: usize) -> Self {
        let mut offsets = [[LinearOffsetList::new(); 6]; 6];
        let stride = stride as isize;

        // For each position in the 6x6 pattern
        for (py, row) in offsets.iter_mut().enumerate() {
            for (px, cell) in row.iter_mut().enumerate() {
                // Find all neighbors of the target color in 5x5 window
                for dy in -(SEARCH_RADIUS as i32)..=(SEARCH_RADIUS as i32) {
                    for dx in -(SEARCH_RADIUS as i32)..=(SEARCH_RADIUS as i32) {
                        // Neighbor position in pattern (with wrapping for lookup)
                        let ny = (py as i32 + dy).rem_euclid(6) as usize;
                        let nx = (px as i32 + dx).rem_euclid(6) as usize;

                        if pattern.pattern[ny][nx] == color {
                            // Pre-compute linear offset
                            let linear_offset = dy as isize * stride + dx as isize;
                            cell.push(linear_offset);
                        }
                    }
                }
                cell.finalize();
            }
        }

        Self { offsets }
    }

    #[inline(always)]
    pub(super) fn get(&self, pattern_y: usize, pattern_x: usize) -> &LinearOffsetList {
        &self.offsets[pattern_y % 6][pattern_x % 6]
    }
}

/// Minimum image width to use SIMD (need enough pixels for vectorized loads).
const MIN_SIMD_WIDTH: usize = 8;

/// Bilinear demosaicing for X-Trans CFA.
///
/// For each pixel, interpolates missing color channels from nearby pixels
/// of the same color. This is a simple approach that works but may produce
/// artifacts in fine detail areas.
///
/// Uses SIMD acceleration on x86_64 (SSE4.1) and aarch64 (NEON) when available.
/// Uses rayon for parallel row processing on large images to avoid false
/// cache sharing (each thread writes to separate cache lines).
///
/// Returns RGB interleaved data: [R0, G0, B0, R1, G1, B1, ...]
#[cfg(target_arch = "x86_64")]
pub fn demosaic_xtrans_bilinear(xtrans: &XTransImage) -> Vec<f32> {
    let red_lookup = NeighborLookup::new(&xtrans.pattern, 0);
    let green_lookup = NeighborLookup::new(&xtrans.pattern, 1);
    let blue_lookup = NeighborLookup::new(&xtrans.pattern, 2);
    let lookups = [&red_lookup, &green_lookup, &blue_lookup];

    let use_parallel = xtrans.width >= MIN_PARALLEL_SIZE && xtrans.height >= MIN_PARALLEL_SIZE;
    let use_simd = common::cpu_features::has_sse4_1() && xtrans.width >= MIN_SIMD_WIDTH;

    if use_parallel || use_simd {
        // Create linear lookups once for SIMD path
        let linear_red = LinearNeighborLookup::new(&xtrans.pattern, 0, xtrans.raw_width);
        let linear_green = LinearNeighborLookup::new(&xtrans.pattern, 1, xtrans.raw_width);
        let linear_blue = LinearNeighborLookup::new(&xtrans.pattern, 2, xtrans.raw_width);
        let linear_lookups = [&linear_red, &linear_green, &linear_blue];

        if use_parallel {
            tracing::info!("Using parallel demosaicing");
            demosaic_parallel_linear(xtrans, &lookups, &linear_lookups, use_simd)
        } else {
            tracing::info!("Using simd demosaicing");
            demosaic_simd_linear(xtrans, &lookups, &linear_lookups)
        }
    } else {
        tracing::info!("Using scalar demosaicing");
        demosaic_scalar(xtrans, &lookups)
    }
}

#[cfg(target_arch = "aarch64")]
pub fn demosaic_xtrans_bilinear(xtrans: &XTransImage) -> Vec<f32> {
    let red_lookup = NeighborLookup::new(&xtrans.pattern, 0);
    let green_lookup = NeighborLookup::new(&xtrans.pattern, 1);
    let blue_lookup = NeighborLookup::new(&xtrans.pattern, 2);
    let lookups = [&red_lookup, &green_lookup, &blue_lookup];

    let use_parallel = xtrans.width >= MIN_PARALLEL_SIZE && xtrans.height >= MIN_PARALLEL_SIZE;
    let use_simd = xtrans.width >= MIN_SIMD_WIDTH;

    if use_parallel || use_simd {
        // Create linear lookups once for SIMD path
        let linear_red = LinearNeighborLookup::new(&xtrans.pattern, 0, xtrans.raw_width);
        let linear_green = LinearNeighborLookup::new(&xtrans.pattern, 1, xtrans.raw_width);
        let linear_blue = LinearNeighborLookup::new(&xtrans.pattern, 2, xtrans.raw_width);
        let linear_lookups = [&linear_red, &linear_green, &linear_blue];

        if use_parallel {
            demosaic_parallel_linear(xtrans, &lookups, &linear_lookups, use_simd)
        } else {
            demosaic_simd_linear(xtrans, &lookups, &linear_lookups)
        }
    } else {
        demosaic_scalar(xtrans, &lookups)
    }
}

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
pub fn demosaic_xtrans_bilinear(xtrans: &XTransImage) -> Vec<f32> {
    let red_lookup = NeighborLookup::new(&xtrans.pattern, 0);
    let green_lookup = NeighborLookup::new(&xtrans.pattern, 1);
    let blue_lookup = NeighborLookup::new(&xtrans.pattern, 2);
    let lookups = [&red_lookup, &green_lookup, &blue_lookup];

    let use_parallel = xtrans.width >= MIN_PARALLEL_SIZE && xtrans.height >= MIN_PARALLEL_SIZE;

    if use_parallel {
        demosaic_parallel(xtrans, &lookups, false)
    } else {
        demosaic_scalar(xtrans, &lookups)
    }
}

/// Parallel row-based demosaicing with pre-computed linear lookups.
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
fn demosaic_parallel_linear(
    xtrans: &XTransImage,
    lookups: &[&NeighborLookup; 3],
    linear_lookups: &[&LinearNeighborLookup; 3],
    use_simd: bool,
) -> Vec<f32> {
    let mut rgb = vec![0.0f32; xtrans.width * xtrans.height * 3];

    let row_stride = xtrans.width * 3;
    rgb.par_chunks_mut(row_stride)
        .enumerate()
        .for_each(|(y, row_rgb)| {
            if use_simd {
                #[cfg(target_arch = "x86_64")]
                unsafe {
                    super::simd_sse4::process_row_simd_linear(
                        xtrans,
                        y,
                        row_rgb,
                        lookups,
                        linear_lookups,
                    );
                }
                #[cfg(target_arch = "aarch64")]
                unsafe {
                    super::simd_neon::process_row_simd_linear(
                        xtrans,
                        y,
                        row_rgb,
                        lookups,
                        linear_lookups,
                    );
                }
            } else {
                process_row(xtrans, y, row_rgb, lookups);
            }
        });

    rgb
}

/// Parallel row-based demosaicing (non-SIMD fallback).
#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
fn demosaic_parallel(
    xtrans: &XTransImage,
    lookups: &[&NeighborLookup; 3],
    _use_simd: bool,
) -> Vec<f32> {
    let mut rgb = vec![0.0f32; xtrans.width * xtrans.height * 3];

    let row_stride = xtrans.width * 3;
    rgb.par_chunks_mut(row_stride)
        .enumerate()
        .for_each(|(y, row_rgb)| {
            process_row(xtrans, y, row_rgb, lookups);
        });

    rgb
}

/// Sequential SIMD demosaicing with pre-computed linear lookups (non-parallel).
#[cfg(target_arch = "x86_64")]
pub(super) fn demosaic_simd_linear(
    xtrans: &XTransImage,
    lookups: &[&NeighborLookup; 3],
    linear_lookups: &[&LinearNeighborLookup; 3],
) -> Vec<f32> {
    let mut rgb = vec![0.0f32; xtrans.width * xtrans.height * 3];

    for y in 0..xtrans.height {
        let row_start = y * xtrans.width * 3;
        let row_rgb = &mut rgb[row_start..row_start + xtrans.width * 3];
        unsafe {
            super::simd_sse4::process_row_simd_linear(xtrans, y, row_rgb, lookups, linear_lookups);
        }
    }

    rgb
}

/// Sequential SIMD demosaicing with pre-computed linear lookups (non-parallel).
#[cfg(target_arch = "aarch64")]
pub(super) fn demosaic_simd_linear(
    xtrans: &XTransImage,
    lookups: &[&NeighborLookup; 3],
    linear_lookups: &[&LinearNeighborLookup; 3],
) -> Vec<f32> {
    let mut rgb = vec![0.0f32; xtrans.width * xtrans.height * 3];

    for y in 0..xtrans.height {
        let row_start = y * xtrans.width * 3;
        let row_rgb = &mut rgb[row_start..row_start + xtrans.width * 3];
        unsafe {
            super::simd_neon::process_row_simd_linear(xtrans, y, row_rgb, lookups, linear_lookups);
        }
    }

    rgb
}

/// Sequential scalar demosaicing for small images.
pub(super) fn demosaic_scalar(xtrans: &XTransImage, lookups: &[&NeighborLookup; 3]) -> Vec<f32> {
    let mut rgb = vec![0.0f32; xtrans.width * xtrans.height * 3];

    for y in 0..xtrans.height {
        let row_start = y * xtrans.width * 3;
        let row_rgb = &mut rgb[row_start..row_start + xtrans.width * 3];
        process_row(xtrans, y, row_rgb, lookups);
    }

    rgb
}

/// Process a single row of the image.
#[inline]
fn process_row(
    xtrans: &XTransImage,
    y: usize,
    row_rgb: &mut [f32],
    lookups: &[&NeighborLookup; 3],
) {
    let raw_y = y + xtrans.top_margin;
    let row_base = raw_y * xtrans.raw_width;

    // Check if this row is in the interior (no vertical bounds checking needed)
    let is_interior_y = raw_y >= SEARCH_RADIUS && raw_y + SEARCH_RADIUS < xtrans.raw_height;

    for x in 0..xtrans.width {
        let raw_x = x + xtrans.left_margin;
        let rgb_idx = x * 3;

        let color = xtrans.pattern.color_at(raw_y, raw_x);
        let val = xtrans.data[row_base + raw_x];

        // Set the known color channel
        row_rgb[rgb_idx + color as usize] = val;

        // Check if this pixel is in the interior (no bounds checking needed)
        let is_interior =
            is_interior_y && raw_x >= SEARCH_RADIUS && raw_x + SEARCH_RADIUS < xtrans.raw_width;

        // Interpolate the other two channels
        for c in 0u8..3 {
            if c != color {
                row_rgb[rgb_idx + c as usize] =
                    interpolate_channel(xtrans, raw_x, raw_y, lookups[c as usize], is_interior);
            }
        }
    }
}

/// Interpolate a color channel from neighboring pixels.
/// When `is_interior` is true, skips bounds checking for better performance.
#[inline(always)]
fn interpolate_channel(
    xtrans: &XTransImage,
    x: usize,
    y: usize,
    lookup: &NeighborLookup,
    is_interior: bool,
) -> f32 {
    let offset_list = lookup.get(y, x);

    if is_interior {
        // Fast path: no bounds checking, use pre-computed inv_len
        let mut sum = 0.0f32;
        for &(dy, dx) in offset_list.as_slice() {
            let ny = (y as i32 + dy) as usize;
            let nx = (x as i32 + dx) as usize;
            sum += xtrans.data[ny * xtrans.raw_width + nx];
        }
        sum * offset_list.inv_len()
    } else {
        // Slow path: with bounds checking for edge pixels
        let mut sum = 0.0f32;
        let mut count = 0u32;
        for &(dy, dx) in offset_list.as_slice() {
            let ny = y as i32 + dy;
            let nx = x as i32 + dx;
            if ny >= 0
                && nx >= 0
                && (ny as usize) < xtrans.raw_height
                && (nx as usize) < xtrans.raw_width
            {
                sum += xtrans.data[ny as usize * xtrans.raw_width + nx as usize];
                count += 1;
            }
        }
        if count > 0 { sum / count as f32 } else { 0.0 }
    }
}

#[cfg(test)]
mod tests {
    use super::super::XTransPattern;
    use super::*;

    // Standard X-Trans pattern
    fn test_pattern() -> XTransPattern {
        XTransPattern::new([
            [1, 0, 1, 1, 2, 1], // G R G G B G
            [2, 1, 2, 0, 1, 0], // B G B R G R
            [1, 2, 1, 1, 0, 1], // G B G G R G
            [1, 2, 1, 1, 0, 1], // G B G G R G
            [0, 1, 0, 2, 1, 2], // R G R B G B
            [1, 0, 1, 1, 2, 1], // G R G G B G
        ])
    }

    #[test]
    fn test_demosaic_output_size() {
        // 12x12 gives us 2 complete X-Trans patterns
        let data = vec![0.5f32; 12 * 12];
        let pattern = test_pattern();
        let xtrans = XTransImage::with_margins(&data, 12, 12, 12, 12, 0, 0, pattern);

        let rgb = demosaic_xtrans_bilinear(&xtrans);
        assert_eq!(rgb.len(), 12 * 12 * 3);
    }

    #[test]
    fn test_demosaic_uniform_gray() {
        // Uniform input should produce roughly uniform output
        let data = vec![0.5f32; 12 * 12];
        let pattern = test_pattern();
        let xtrans = XTransImage::with_margins(&data, 12, 12, 12, 12, 0, 0, pattern);

        let rgb = demosaic_xtrans_bilinear(&xtrans);

        for &v in &rgb {
            assert!((v - 0.5).abs() < 0.01, "Expected ~0.5, got {}", v);
        }
    }

    #[test]
    fn test_demosaic_preserves_known_channel() {
        // At a known pixel position, the original value should be preserved
        let mut data = vec![0.0f32; 12 * 12];
        // Set position (0, 1) which is Red in our test pattern
        data[1] = 1.0;

        let pattern = test_pattern();
        let xtrans = XTransImage::with_margins(&data, 12, 12, 12, 12, 0, 0, pattern);

        let rgb = demosaic_xtrans_bilinear(&xtrans);

        // Position (0, 1) -> RGB index 1*3 = 3, Red channel is +0
        let red_at_0_1 = rgb[3];
        assert!(
            (red_at_0_1 - 1.0).abs() < 0.001,
            "Red channel at (0,1) should be 1.0, got {}",
            red_at_0_1
        );
    }

    #[test]
    fn test_demosaic_with_margins() {
        // Test that margins are handled correctly
        let data = vec![0.5f32; 18 * 18];
        let pattern = test_pattern();
        let xtrans = XTransImage::with_margins(&data, 18, 18, 12, 12, 3, 3, pattern);

        let rgb = demosaic_xtrans_bilinear(&xtrans);
        assert_eq!(rgb.len(), 12 * 12 * 3);

        // All values should be approximately 0.5
        for &v in &rgb {
            assert!((v - 0.5).abs() < 0.01, "Expected ~0.5, got {}", v);
        }
    }

    #[test]
    fn test_interpolate_channel_finds_neighbors() {
        let data = vec![0.5f32; 12 * 12];
        let pattern = test_pattern();
        let xtrans = XTransImage::with_margins(&data, 12, 12, 12, 12, 0, 0, pattern);

        // Test interpolation at center of image using the safe path
        let red_lookup = NeighborLookup::new(&xtrans.pattern, 0);
        let green_lookup = NeighborLookup::new(&xtrans.pattern, 1);
        let blue_lookup = NeighborLookup::new(&xtrans.pattern, 2);

        let val = interpolate_channel(&xtrans, 6, 6, &red_lookup, false); // Red
        assert!(val > 0.0, "Should find red neighbors");

        let val = interpolate_channel(&xtrans, 6, 6, &green_lookup, false); // Green
        assert!(val > 0.0, "Should find green neighbors");

        let val = interpolate_channel(&xtrans, 6, 6, &blue_lookup, false); // Blue
        assert!(val > 0.0, "Should find blue neighbors");
    }

    #[test]
    fn test_parallel_vs_scalar_consistency() {
        // Create an image large enough to trigger parallel path
        // Use demosaic_xtrans_bilinear which will choose parallel/SIMD path
        let size = 256;
        let data: Vec<f32> = (0..size * size).map(|i| (i % 256) as f32 / 255.0).collect();
        let pattern = test_pattern();
        let xtrans = XTransImage::with_margins(&data, size, size, size, size, 0, 0, pattern);

        let red_lookup = NeighborLookup::new(&xtrans.pattern, 0);
        let green_lookup = NeighborLookup::new(&xtrans.pattern, 1);
        let blue_lookup = NeighborLookup::new(&xtrans.pattern, 2);
        let lookups = [&red_lookup, &green_lookup, &blue_lookup];

        // Compare optimized path against scalar
        let optimized_result = demosaic_xtrans_bilinear(&xtrans);
        let scalar_result = demosaic_scalar(&xtrans, &lookups);

        assert_eq!(optimized_result.len(), scalar_result.len());
        for (i, (p, s)) in optimized_result
            .iter()
            .zip(scalar_result.iter())
            .enumerate()
        {
            assert!(
                (*p - *s).abs() < 1e-5,
                "Mismatch at index {}: optimized={}, scalar={}",
                i,
                p,
                s
            );
        }
    }

    #[test]
    fn test_fast_vs_safe_interpolation_consistency() {
        // Test that fast and safe paths produce identical results for interior pixels
        let data = vec![0.5f32; 24 * 24];
        let pattern = test_pattern();
        let xtrans = XTransImage::with_margins(&data, 24, 24, 24, 24, 0, 0, pattern);

        let red_lookup = NeighborLookup::new(&xtrans.pattern, 0);

        // Test at interior position (well within bounds)
        let x = 12;
        let y = 12;
        let fast = interpolate_channel(&xtrans, x, y, &red_lookup, true);
        let safe = interpolate_channel(&xtrans, x, y, &red_lookup, false);

        assert!(
            (fast - safe).abs() < 1e-6,
            "Fast and safe paths differ: fast={}, safe={}",
            fast,
            safe
        );
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn test_simd_vs_scalar_consistency() {
        use crate::astro_image::demosaic::xtrans::simd_sse4;

        // Test that SIMD and scalar implementations produce identical results
        // Use a size that exercises the SIMD path
        let size = 64;
        let data: Vec<f32> = (0..size * size).map(|i| (i % 256) as f32 / 255.0).collect();
        let pattern = test_pattern();
        let xtrans = XTransImage::with_margins(&data, size, size, size, size, 0, 0, pattern);

        let red_lookup = NeighborLookup::new(&xtrans.pattern, 0);
        let green_lookup = NeighborLookup::new(&xtrans.pattern, 1);
        let blue_lookup = NeighborLookup::new(&xtrans.pattern, 2);
        let lookups = [&red_lookup, &green_lookup, &blue_lookup];

        // Create linear lookups
        let linear_red = LinearNeighborLookup::new(&xtrans.pattern, 0, xtrans.raw_width);
        let linear_green = LinearNeighborLookup::new(&xtrans.pattern, 1, xtrans.raw_width);
        let linear_blue = LinearNeighborLookup::new(&xtrans.pattern, 2, xtrans.raw_width);
        let linear_lookups = [&linear_red, &linear_green, &linear_blue];

        // Get scalar result
        let scalar_result = demosaic_scalar(&xtrans, &lookups);

        // Get SIMD result if SSE4.1 is available
        if common::cpu_features::has_sse4_1() {
            let mut simd_result = vec![0.0f32; size * size * 3];
            for y in 0..size {
                let row_start = y * size * 3;
                let row_rgb = &mut simd_result[row_start..row_start + size * 3];
                unsafe {
                    simd_sse4::process_row_simd_linear(
                        &xtrans,
                        y,
                        row_rgb,
                        &lookups,
                        &linear_lookups,
                    );
                }
            }

            assert_eq!(simd_result.len(), scalar_result.len());
            for (i, (simd, scalar)) in simd_result.iter().zip(scalar_result.iter()).enumerate() {
                assert!(
                    (*simd - *scalar).abs() < 1e-5,
                    "SIMD vs scalar mismatch at index {}: simd={}, scalar={}",
                    i,
                    simd,
                    scalar
                );
            }
        }
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn test_simd_parallel_vs_scalar_consistency() {
        // Test that parallel SIMD produces the same result as scalar
        let size = 256;
        let data: Vec<f32> = (0..size * size).map(|i| (i % 256) as f32 / 255.0).collect();
        let pattern = test_pattern();
        let xtrans = XTransImage::with_margins(&data, size, size, size, size, 0, 0, pattern);

        let red_lookup = NeighborLookup::new(&xtrans.pattern, 0);
        let green_lookup = NeighborLookup::new(&xtrans.pattern, 1);
        let blue_lookup = NeighborLookup::new(&xtrans.pattern, 2);
        let lookups = [&red_lookup, &green_lookup, &blue_lookup];

        let scalar_result = demosaic_scalar(&xtrans, &lookups);

        if common::cpu_features::has_sse4_1() {
            // demosaic_xtrans_bilinear will use parallel SIMD path for large images
            let simd_parallel_result = demosaic_xtrans_bilinear(&xtrans);

            assert_eq!(simd_parallel_result.len(), scalar_result.len());
            for (i, (simd, scalar)) in simd_parallel_result
                .iter()
                .zip(scalar_result.iter())
                .enumerate()
            {
                assert!(
                    (*simd - *scalar).abs() < 1e-5,
                    "SIMD parallel vs scalar mismatch at index {}: simd={}, scalar={}",
                    i,
                    simd,
                    scalar
                );
            }
        }
    }

    #[test]
    fn test_demosaic_with_varied_data() {
        // Test with more varied data patterns to catch edge cases
        let size = 48;
        // Create a gradient pattern
        let data: Vec<f32> = (0..size * size)
            .map(|i| {
                let x = i % size;
                let y = i / size;
                ((x + y) % 256) as f32 / 255.0
            })
            .collect();
        let pattern = test_pattern();
        let xtrans = XTransImage::with_margins(&data, size, size, size, size, 0, 0, pattern);

        let rgb = demosaic_xtrans_bilinear(&xtrans);
        assert_eq!(rgb.len(), size * size * 3);

        // Verify no NaN or infinite values
        for (i, &v) in rgb.iter().enumerate() {
            assert!(v.is_finite(), "Non-finite value at index {}: {}", i, v);
            assert!(v >= 0.0, "Negative value at index {}: {}", i, v);
        }
    }

    #[test]
    fn test_demosaic_all_zeros() {
        // All zeros input should produce all zeros output
        let data = vec![0.0f32; 12 * 12];
        let pattern = test_pattern();
        let xtrans = XTransImage::with_margins(&data, 12, 12, 12, 12, 0, 0, pattern);

        let rgb = demosaic_xtrans_bilinear(&xtrans);

        for (i, &v) in rgb.iter().enumerate() {
            assert!(v.abs() < 1e-6, "Expected 0.0 at index {}, got {}", i, v);
        }
    }

    #[test]
    fn test_demosaic_all_max() {
        // All 1.0 input should produce all 1.0 output
        let data = vec![1.0f32; 12 * 12];
        let pattern = test_pattern();
        let xtrans = XTransImage::with_margins(&data, 12, 12, 12, 12, 0, 0, pattern);

        let rgb = demosaic_xtrans_bilinear(&xtrans);

        for (i, &v) in rgb.iter().enumerate() {
            assert!(
                (v - 1.0).abs() < 0.01,
                "Expected ~1.0 at index {}, got {}",
                i,
                v
            );
        }
    }

    #[test]
    fn test_demosaic_no_nan_or_infinity() {
        // Test with extreme gradient values that might cause numerical issues
        let size = 24;
        let mut data = vec![0.0f32; size * size];

        // Create alternating extreme values
        for (i, val) in data.iter_mut().enumerate() {
            *val = if i % 2 == 0 { 0.0 } else { 1.0 };
        }

        let pattern = test_pattern();
        let xtrans = XTransImage::with_margins(&data, size, size, size, size, 0, 0, pattern);

        let rgb = demosaic_xtrans_bilinear(&xtrans);

        for (i, &v) in rgb.iter().enumerate() {
            assert!(!v.is_nan(), "NaN at index {}", i);
            assert!(v.is_finite(), "Infinite at index {}", i);
        }
    }

    #[test]
    fn test_demosaic_corner_pixels() {
        // Test that corner pixels are correctly interpolated
        let size = 12;
        let data = vec![0.5f32; size * size];
        let pattern = test_pattern();
        let xtrans = XTransImage::with_margins(&data, size, size, size, size, 0, 0, pattern);

        let rgb = demosaic_xtrans_bilinear(&xtrans);

        // Check corners: (0,0), (0,w-1), (h-1,0), (h-1,w-1)
        let corners = [(0, 0), (0, size - 1), (size - 1, 0), (size - 1, size - 1)];

        for (y, x) in corners {
            let idx = (y * size + x) * 3;
            for c in 0..3 {
                let v = rgb[idx + c];
                assert!(!v.is_nan(), "NaN at corner ({}, {}), channel {}", y, x, c);
                assert!(
                    v.is_finite(),
                    "Infinite at corner ({}, {}), channel {}",
                    y,
                    x,
                    c
                );
                // With uniform 0.5 input, all outputs should be approximately 0.5
                assert!(
                    (v - 0.5).abs() < 0.1,
                    "Unexpected value {} at corner ({}, {}), channel {}",
                    v,
                    y,
                    x,
                    c
                );
            }
        }
    }

    #[test]
    fn test_demosaic_asymmetric_margins() {
        // Test with non-symmetric margins
        let raw_w = 18;
        let raw_h = 15;
        let left = 2;
        let top = 1;
        let out_w = 12;
        let out_h = 12;

        let data = vec![0.5f32; raw_w * raw_h];
        let pattern = test_pattern();
        let xtrans =
            XTransImage::with_margins(&data, raw_w, raw_h, out_w, out_h, left, top, pattern);

        let rgb = demosaic_xtrans_bilinear(&xtrans);
        assert_eq!(rgb.len(), out_w * out_h * 3);

        // Verify no NaN or infinite values
        for (i, &v) in rgb.iter().enumerate() {
            assert!(v.is_finite(), "Non-finite at index {}", i);
        }
    }

    #[test]
    fn test_demosaic_non_square_image() {
        // Test with wide image
        let data = vec![0.5f32; 24 * 12];
        let pattern = test_pattern();
        let xtrans = XTransImage::with_margins(&data, 24, 12, 24, 12, 0, 0, pattern);

        let rgb = demosaic_xtrans_bilinear(&xtrans);
        assert_eq!(rgb.len(), 24 * 12 * 3);

        // Test with tall image
        let data = vec![0.5f32; 12 * 24];
        let pattern = test_pattern();
        let xtrans = XTransImage::with_margins(&data, 12, 24, 12, 24, 0, 0, pattern);

        let rgb = demosaic_xtrans_bilinear(&xtrans);
        assert_eq!(rgb.len(), 12 * 24 * 3);
    }

    #[test]
    fn test_demosaic_preserves_green_at_green_pixel() {
        // Position (0,0) is Green in test pattern
        // Verify it is preserved exactly
        let size = 12;
        let mut data = vec![0.0f32; size * size];
        data[0] = 0.75; // (0,0) is Green

        let pattern = test_pattern();
        let xtrans = XTransImage::with_margins(&data, size, size, size, size, 0, 0, pattern);

        let rgb = demosaic_xtrans_bilinear(&xtrans);

        // Position (0,0) -> RGB index 0, Green channel is index 1
        let green_at_0_0 = rgb[1];
        assert!(
            (green_at_0_0 - 0.75).abs() < 0.001,
            "Green channel at (0,0) should be 0.75, got {}",
            green_at_0_0
        );
    }

    #[test]
    fn test_demosaic_preserves_blue_at_blue_pixel() {
        // Position (1,0) is Blue in test pattern
        let size = 12;
        let mut data = vec![0.0f32; size * size];
        data[size] = 0.8; // (1,0) is Blue

        let pattern = test_pattern();
        let xtrans = XTransImage::with_margins(&data, size, size, size, size, 0, 0, pattern);

        let rgb = demosaic_xtrans_bilinear(&xtrans);

        // Position (1,0) -> RGB index (1*12+0)*3 = 36, Blue channel is +2 = 38
        let blue_at_1_0 = rgb[38];
        assert!(
            (blue_at_1_0 - 0.8).abs() < 0.001,
            "Blue channel at (1,0) should be 0.8, got {}",
            blue_at_1_0
        );
    }

    #[test]
    fn test_neighbor_lookup_finds_correct_colors() {
        // Verify that NeighborLookup finds the correct neighbors
        let pattern = test_pattern();

        // For Red color (0)
        let red_lookup = NeighborLookup::new(&pattern, 0);

        // At position (0,0) which is Green, check that red neighbors are found
        let offsets = red_lookup.get(0, 0);
        assert!(
            !offsets.as_slice().is_empty(),
            "Should find red neighbors from position (0,0)"
        );

        // For Green color (1)
        let green_lookup = NeighborLookup::new(&pattern, 1);
        let offsets = green_lookup.get(0, 0);
        // (0,0) is Green itself, so (0,0) offset should be in the list
        assert!(
            offsets.as_slice().contains(&(0, 0)),
            "Green lookup at Green position should include (0,0)"
        );

        // For Blue color (2)
        let blue_lookup = NeighborLookup::new(&pattern, 2);
        let offsets = blue_lookup.get(0, 0);
        assert!(
            !offsets.as_slice().is_empty(),
            "Should find blue neighbors from position (0,0)"
        );
    }

    #[test]
    fn test_linear_neighbor_lookup_consistency() {
        // Verify that LinearNeighborLookup produces consistent results with NeighborLookup
        let pattern = test_pattern();
        let stride = 24usize;

        let neighbor_lookup = NeighborLookup::new(&pattern, 0);
        let linear_lookup = LinearNeighborLookup::new(&pattern, 0, stride);

        // Check several positions
        for py in 0..6 {
            for px in 0..6 {
                let neighbor_offsets = neighbor_lookup.get(py, px);
                let linear_offsets = linear_lookup.get(py, px);

                assert_eq!(
                    neighbor_offsets.as_slice().len(),
                    linear_offsets.as_slice().len(),
                    "Length mismatch at ({}, {})",
                    py,
                    px
                );

                // Verify linear offsets match dy*stride + dx
                for (i, &(dy, dx)) in neighbor_offsets.as_slice().iter().enumerate() {
                    let expected_linear = dy as isize * stride as isize + dx as isize;
                    assert_eq!(
                        linear_offsets.as_slice()[i],
                        expected_linear,
                        "Linear offset mismatch at ({}, {})[{}]: expected {}, got {}",
                        py,
                        px,
                        i,
                        expected_linear,
                        linear_offsets.as_slice()[i]
                    );
                }
            }
        }
    }

    #[test]
    fn test_gradient_pattern_interpolation() {
        // Test that gradient patterns interpolate smoothly
        let size = 24;
        let data: Vec<f32> = (0..size * size)
            .map(|i| {
                let x = i % size;
                x as f32 / (size - 1) as f32
            })
            .collect();
        let pattern = test_pattern();
        let xtrans = XTransImage::with_margins(&data, size, size, size, size, 0, 0, pattern);

        let rgb = demosaic_xtrans_bilinear(&xtrans);

        // Check that values increase from left to right
        for y in 0..size {
            let mut prev_sum = 0.0f32;
            for x in 0..size {
                let idx = (y * size + x) * 3;
                let sum: f32 = rgb[idx..idx + 3].iter().sum();
                if x > 0 {
                    // Allow small tolerance for interpolation differences
                    assert!(
                        sum >= prev_sum - 0.5,
                        "Gradient should generally increase: row {}, col {} has sum {} < prev {}",
                        y,
                        x,
                        sum,
                        prev_sum
                    );
                }
                prev_sum = sum;
            }
        }
    }
}
