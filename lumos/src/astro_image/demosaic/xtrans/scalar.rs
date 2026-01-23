//! Scalar implementation of X-Trans bilinear demosaicing.
//!
//! This is a basic bilinear interpolation for X-Trans sensors.
//! It's simpler and faster than advanced algorithms like Markesteijn,
//! but produces lower quality results (may show artifacts).
//!
//! # Optimizations
//! - Pre-computed neighbor offset lookup tables per pattern position
//! - Separate fast path for interior pixels (no bounds checking)
//! - Row base index pre-computation
//! - Parallel processing via rayon for large images

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
    let use_simd = is_x86_feature_detected!("sse4.1") && xtrans.width >= MIN_SIMD_WIDTH;

    if use_parallel {
        tracing::info!("Using parallel demosaicing");
        demosaic_parallel(xtrans, &lookups, use_simd)
    } else if use_simd {
        tracing::info!("Using simd demosaicing");
        demosaic_simd(xtrans, &lookups)
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

    if use_parallel {
        demosaic_parallel(xtrans, &lookups, use_simd)
    } else if use_simd {
        demosaic_simd(xtrans, &lookups)
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

/// Parallel row-based demosaicing.
/// Processes rows in parallel using rayon, with each thread writing to its own
/// row buffer to avoid false cache sharing.
fn demosaic_parallel(
    xtrans: &XTransImage,
    lookups: &[&NeighborLookup; 3],
    use_simd: bool,
) -> Vec<f32> {
    let mut rgb = vec![0.0f32; xtrans.width * xtrans.height * 3];

    // Process rows in parallel - each row is a separate chunk
    // This ensures no false sharing since each thread writes to different cache lines
    let row_stride = xtrans.width * 3;
    rgb.par_chunks_mut(row_stride)
        .enumerate()
        .for_each(|(y, row_rgb)| {
            if use_simd {
                #[cfg(target_arch = "x86_64")]
                unsafe {
                    super::simd_sse4::process_row_simd_sse4(xtrans, y, row_rgb, lookups);
                }
                #[cfg(target_arch = "aarch64")]
                unsafe {
                    super::simd_neon::process_row_simd_neon(xtrans, y, row_rgb, lookups);
                }
                #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
                process_row(xtrans, y, row_rgb, lookups);
            } else {
                process_row(xtrans, y, row_rgb, lookups);
            }
        });

    rgb
}

/// Sequential SIMD demosaicing for medium-sized images.
#[cfg(target_arch = "x86_64")]
pub(super) fn demosaic_simd(xtrans: &XTransImage, lookups: &[&NeighborLookup; 3]) -> Vec<f32> {
    let mut rgb = vec![0.0f32; xtrans.width * xtrans.height * 3];

    for y in 0..xtrans.height {
        let row_start = y * xtrans.width * 3;
        let row_rgb = &mut rgb[row_start..row_start + xtrans.width * 3];
        unsafe {
            super::simd_sse4::process_row_simd_sse4(xtrans, y, row_rgb, lookups);
        }
    }

    rgb
}

/// Sequential SIMD demosaicing for medium-sized images.
#[cfg(target_arch = "aarch64")]
fn demosaic_simd(xtrans: &XTransImage, lookups: &[&NeighborLookup; 3]) -> Vec<f32> {
    let mut rgb = vec![0.0f32; xtrans.width * xtrans.height * 3];

    for y in 0..xtrans.height {
        let row_start = y * xtrans.width * 3;
        let row_rgb = &mut rgb[row_start..row_start + xtrans.width * 3];
        unsafe {
            super::simd_neon::process_row_simd_neon(xtrans, y, row_rgb, lookups);
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
        if is_interior {
            // Fast path: no bounds checking
            for c in 0u8..3 {
                if c != color {
                    row_rgb[rgb_idx + c as usize] =
                        interpolate_channel_fast(xtrans, raw_x, raw_y, lookups[c as usize]);
                }
            }
        } else {
            // Slow path: with bounds checking for edge pixels
            for c in 0u8..3 {
                if c != color {
                    row_rgb[rgb_idx + c as usize] =
                        interpolate_channel_safe(xtrans, raw_x, raw_y, lookups[c as usize]);
                }
            }
        }
    }
}

/// Fast interpolation for interior pixels (no bounds checking).
#[inline(always)]
fn interpolate_channel_fast(
    xtrans: &XTransImage,
    x: usize,
    y: usize,
    lookup: &NeighborLookup,
) -> f32 {
    let offset_list = lookup.get(y, x);
    let mut sum = 0.0f32;

    for &(dy, dx) in offset_list.as_slice() {
        let ny = (y as i32 + dy) as usize;
        let nx = (x as i32 + dx) as usize;
        sum += xtrans.data[ny * xtrans.raw_width + nx];
    }

    sum * offset_list.inv_len()
}

/// Safe interpolation with bounds checking for edge pixels.
#[inline]
fn interpolate_channel_safe(
    xtrans: &XTransImage,
    x: usize,
    y: usize,
    lookup: &NeighborLookup,
) -> f32 {
    let offset_list = lookup.get(y, x);
    let mut sum = 0.0f32;
    let mut count = 0u32;

    for &(dy, dx) in offset_list.as_slice() {
        let ny = y as i32 + dy;
        let nx = x as i32 + dx;

        // Bounds check
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

        let val = interpolate_channel_safe(&xtrans, 6, 6, &red_lookup); // Red
        assert!(val > 0.0, "Should find red neighbors");

        let val = interpolate_channel_safe(&xtrans, 6, 6, &green_lookup); // Green
        assert!(val > 0.0, "Should find green neighbors");

        let val = interpolate_channel_safe(&xtrans, 6, 6, &blue_lookup); // Blue
        assert!(val > 0.0, "Should find blue neighbors");
    }

    #[test]
    fn test_parallel_vs_scalar_consistency() {
        // Create an image large enough to trigger parallel path
        let size = 256;
        let data: Vec<f32> = (0..size * size).map(|i| (i % 256) as f32 / 255.0).collect();
        let pattern = test_pattern();
        let xtrans = XTransImage::with_margins(&data, size, size, size, size, 0, 0, pattern);

        let red_lookup = NeighborLookup::new(&xtrans.pattern, 0);
        let green_lookup = NeighborLookup::new(&xtrans.pattern, 1);
        let blue_lookup = NeighborLookup::new(&xtrans.pattern, 2);
        let lookups = [&red_lookup, &green_lookup, &blue_lookup];

        let parallel_result = demosaic_parallel(&xtrans, &lookups, false);
        let scalar_result = demosaic_scalar(&xtrans, &lookups);

        assert_eq!(parallel_result.len(), scalar_result.len());
        for (i, (p, s)) in parallel_result.iter().zip(scalar_result.iter()).enumerate() {
            assert!(
                (p - s).abs() < 1e-6,
                "Mismatch at index {}: parallel={}, scalar={}",
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
        let fast = interpolate_channel_fast(&xtrans, x, y, &red_lookup);
        let safe = interpolate_channel_safe(&xtrans, x, y, &red_lookup);

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

        // Get scalar result
        let scalar_result = demosaic_scalar(&xtrans, &lookups);

        // Get SIMD result if SSE4.1 is available
        if is_x86_feature_detected!("sse4.1") {
            let mut simd_result = vec![0.0f32; size * size * 3];
            for y in 0..size {
                let row_start = y * size * 3;
                let row_rgb = &mut simd_result[row_start..row_start + size * 3];
                unsafe {
                    simd_sse4::process_row_simd_sse4(&xtrans, y, row_rgb, &lookups);
                }
            }

            assert_eq!(simd_result.len(), scalar_result.len());
            for (i, (simd, scalar)) in simd_result.iter().zip(scalar_result.iter()).enumerate() {
                assert!(
                    (simd - scalar).abs() < 1e-5,
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

        if is_x86_feature_detected!("sse4.1") {
            let simd_parallel_result = demosaic_parallel(&xtrans, &lookups, true);

            assert_eq!(simd_parallel_result.len(), scalar_result.len());
            for (i, (simd, scalar)) in simd_parallel_result
                .iter()
                .zip(scalar_result.iter())
                .enumerate()
            {
                assert!(
                    (simd - scalar).abs() < 1e-5,
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
}
