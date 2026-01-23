//! Bayer CFA demosaicing module.
//!
//! Provides bilinear demosaicing of Bayer CFA patterns to RGB images.
//! Uses SIMD acceleration on x86_64 (SSE3) and aarch64 (NEON) when available.
//!
//! # Optimizations
//! - **Rayon parallelization**: Row-based parallel processing for multi-core systems
//! - **Tile-based processing**: 64x64 tiles for better cache locality
//! - **Vectorized CFA lookup**: Pre-computed lookup tables for 2x2 pattern blocks
//! - **Pattern specialization**: Separate code paths per CFA pattern to eliminate branching

mod bayer;

use rayon::prelude::*;

/// Minimum image size to use parallel processing (avoids overhead for small images).
const MIN_PARALLEL_SIZE: usize = 128;

/// Bayer CFA (Color Filter Array) pattern.
/// Represents the 2x2 pattern of color filters on the sensor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum CfaPattern {
    /// RGGB: Red at (0,0), Green at (0,1) and (1,0), Blue at (1,1)
    Rggb,
    /// BGGR: Blue at (0,0), Green at (0,1) and (1,0), Red at (1,1)
    Bggr,
    /// GRBG: Green at (0,0), Red at (0,1), Blue at (1,0), Green at (1,1)
    Grbg,
    /// GBRG: Green at (0,0), Blue at (0,1), Red at (1,0), Green at (1,1)
    Gbrg,
}

impl CfaPattern {
    /// Get color index at position (y, x) in the Bayer pattern.
    /// Returns: 0=Red, 1=Green, 2=Blue
    #[cfg(test)]
    #[inline(always)]
    pub fn color_at(&self, y: usize, x: usize) -> usize {
        let row = y & 1;
        let col = x & 1;
        match self {
            CfaPattern::Rggb => [0, 1, 1, 2][(row << 1) | col],
            CfaPattern::Bggr => [2, 1, 1, 0][(row << 1) | col],
            CfaPattern::Grbg => [1, 0, 2, 1][(row << 1) | col],
            CfaPattern::Gbrg => [1, 2, 0, 1][(row << 1) | col],
        }
    }

    /// Check if red is on the same row as a green pixel at position (y, x).
    /// Used to determine interpolation direction for green pixels.
    #[inline(always)]
    pub fn red_in_row(&self, y: usize) -> bool {
        match self {
            CfaPattern::Rggb | CfaPattern::Grbg => (y & 1) == 0,
            CfaPattern::Bggr | CfaPattern::Gbrg => (y & 1) == 1,
        }
    }

    /// Get the 2x2 color pattern as [row0_col0, row0_col1, row1_col0, row1_col1].
    /// Values: 0=Red, 1=Green, 2=Blue
    #[inline(always)]
    pub fn pattern_2x2(&self) -> [usize; 4] {
        match self {
            CfaPattern::Rggb => [0, 1, 1, 2],
            CfaPattern::Bggr => [2, 1, 1, 0],
            CfaPattern::Grbg => [1, 0, 2, 1],
            CfaPattern::Gbrg => [1, 2, 0, 1],
        }
    }
}

/// Raw Bayer image data with metadata needed for demosaicing.
#[derive(Debug)]
pub struct BayerImage<'a> {
    /// Raw Bayer pixel data (normalized to 0.0-1.0)
    pub data: &'a [f32],
    /// Width of the raw data buffer
    pub raw_width: usize,
    /// Height of the raw data buffer
    pub raw_height: usize,
    /// Width of the active/output image area
    pub width: usize,
    /// Height of the active/output image area
    pub height: usize,
    /// Top margin (offset from raw to active area)
    pub top_margin: usize,
    /// Left margin (offset from raw to active area)
    pub left_margin: usize,
    /// CFA pattern
    pub cfa: CfaPattern,
}

impl<'a> BayerImage<'a> {
    /// Create a BayerImage with margins (libraw style).
    ///
    /// # Panics
    /// Panics if:
    /// - `data.len() != raw_width * raw_height`
    /// - `top_margin + height > raw_height`
    /// - `left_margin + width > raw_width`
    /// - `width == 0` or `height == 0`
    #[allow(clippy::too_many_arguments)]
    pub fn with_margins(
        data: &'a [f32],
        raw_width: usize,
        raw_height: usize,
        width: usize,
        height: usize,
        top_margin: usize,
        left_margin: usize,
        cfa: CfaPattern,
    ) -> Self {
        assert!(
            width > 0 && height > 0,
            "Output dimensions must be non-zero: {}x{}",
            width,
            height
        );
        assert!(
            raw_width > 0 && raw_height > 0,
            "Raw dimensions must be non-zero: {}x{}",
            raw_width,
            raw_height
        );
        assert!(
            data.len() == raw_width * raw_height,
            "Data length {} doesn't match raw dimensions {}x{}={}",
            data.len(),
            raw_width,
            raw_height,
            raw_width * raw_height
        );
        assert!(
            top_margin + height <= raw_height,
            "Top margin {} + height {} exceeds raw height {}",
            top_margin,
            height,
            raw_height
        );
        assert!(
            left_margin + width <= raw_width,
            "Left margin {} + width {} exceeds raw width {}",
            left_margin,
            width,
            raw_width
        );

        Self {
            data,
            raw_width,
            raw_height,
            width,
            height,
            top_margin,
            left_margin,
            cfa,
        }
    }
}

/// Simple bilinear demosaicing of Bayer CFA data to RGB.
///
/// Takes a Bayer pattern image and produces an RGB image using bilinear interpolation.
/// The output has 3 channels (RGB) interleaved: [R0, G0, B0, R1, G1, B1, ...].
///
/// Uses SIMD acceleration on x86_64 (SSE3) and aarch64 (NEON) when available,
/// with automatic fallback to scalar implementation.
///
/// # Optimizations
/// - Uses rayon for parallel row/tile processing on large images
/// - Tile-based processing for better cache locality
/// - Vectorized CFA pattern lookup
#[cfg(target_arch = "x86_64")]
pub fn demosaic_bilinear(bayer: &BayerImage) -> Vec<f32> {
    let use_parallel = bayer.width >= MIN_PARALLEL_SIZE && bayer.height >= MIN_PARALLEL_SIZE;
    let use_simd = is_x86_feature_detected!("sse3") && bayer.width >= 8 && bayer.height >= 4;

    if use_parallel {
        demosaic_parallel(bayer, use_simd)
    } else if use_simd {
        unsafe { bayer::simd_sse3::demosaic_bilinear_sse3(bayer) }
    } else {
        bayer::scalar::demosaic_bilinear_scalar(bayer)
    }
}

#[cfg(target_arch = "aarch64")]
pub fn demosaic_bilinear(bayer: &BayerImage) -> Vec<f32> {
    let use_parallel = bayer.width >= MIN_PARALLEL_SIZE && bayer.height >= MIN_PARALLEL_SIZE;
    let use_simd = bayer.width >= 8 && bayer.height >= 4;

    if use_parallel {
        demosaic_parallel(bayer, use_simd)
    } else if use_simd {
        unsafe { bayer::simd_neon::demosaic_bilinear_neon(bayer) }
    } else {
        bayer::scalar::demosaic_bilinear_scalar(bayer)
    }
}

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
pub fn demosaic_bilinear(bayer: &BayerImage) -> Vec<f32> {
    let use_parallel = bayer.width >= MIN_PARALLEL_SIZE && bayer.height >= MIN_PARALLEL_SIZE;

    if use_parallel {
        demosaic_parallel(bayer, false)
    } else {
        bayer::scalar::demosaic_bilinear_scalar(bayer)
    }
}

/// Parallel row-based demosaicing.
/// Processes rows in parallel using rayon.
fn demosaic_parallel(bayer: &BayerImage, use_simd: bool) -> Vec<f32> {
    let mut rgb = vec![0.0f32; bayer.width * bayer.height * 3];

    // Process rows in parallel
    let row_stride = bayer.width * 3;
    rgb.par_chunks_mut(row_stride)
        .enumerate()
        .for_each(|(y, row_rgb)| {
            if use_simd {
                #[cfg(target_arch = "x86_64")]
                unsafe {
                    process_row_simd_sse3(bayer, row_rgb, y);
                }
                #[cfg(target_arch = "aarch64")]
                unsafe {
                    process_row_simd_neon(bayer, row_rgb, y);
                }
                #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
                process_row_scalar(bayer, row_rgb, y);
            } else {
                process_row_scalar(bayer, row_rgb, y);
            }
        });

    rgb
}

/// Process a single row with scalar code.
#[inline]
fn process_row_scalar(bayer: &BayerImage, row_rgb: &mut [f32], y: usize) {
    let raw_y = y + bayer.top_margin;
    let pattern = bayer.cfa.pattern_2x2();
    let red_in_row = bayer.cfa.red_in_row(raw_y);
    let row_pattern_idx = (raw_y & 1) << 1;

    for x in 0..bayer.width {
        let raw_x = x + bayer.left_margin;
        let rgb_idx = x * 3;

        // Compute color from pre-computed pattern
        let color = pattern[row_pattern_idx | (raw_x & 1)];
        let val = bayer.data[raw_y * bayer.raw_width + raw_x];

        match color {
            0 => {
                // Red pixel
                row_rgb[rgb_idx] = val;
                row_rgb[rgb_idx + 1] = bayer::scalar::interpolate_cross(bayer, raw_x, raw_y);
                row_rgb[rgb_idx + 2] = bayer::scalar::interpolate_diagonal(bayer, raw_x, raw_y);
            }
            1 => {
                // Green pixel
                if red_in_row {
                    row_rgb[rgb_idx] = bayer::scalar::interpolate_horizontal(bayer, raw_x, raw_y);
                    row_rgb[rgb_idx + 2] = bayer::scalar::interpolate_vertical(bayer, raw_x, raw_y);
                } else {
                    row_rgb[rgb_idx] = bayer::scalar::interpolate_vertical(bayer, raw_x, raw_y);
                    row_rgb[rgb_idx + 2] =
                        bayer::scalar::interpolate_horizontal(bayer, raw_x, raw_y);
                }
                row_rgb[rgb_idx + 1] = val;
            }
            2 => {
                // Blue pixel
                row_rgb[rgb_idx] = bayer::scalar::interpolate_diagonal(bayer, raw_x, raw_y);
                row_rgb[rgb_idx + 1] = bayer::scalar::interpolate_cross(bayer, raw_x, raw_y);
                row_rgb[rgb_idx + 2] = val;
            }
            _ => unreachable!(),
        }
    }
}

/// Process a single row with SSE3 SIMD.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse3")]
unsafe fn process_row_simd_sse3(bayer: &BayerImage, row_rgb: &mut [f32], y: usize) {
    use std::arch::x86_64::*;

    let raw_y = y + bayer.top_margin;
    let pattern = bayer.cfa.pattern_2x2();
    let red_in_row = bayer.cfa.red_in_row(raw_y);
    let row_pattern_idx = (raw_y & 1) << 1;

    // Check if we have enough margin for safe SIMD access
    let can_simd_interior =
        raw_y > 0 && raw_y + 1 < bayer.raw_height && bayer.left_margin > 0 && bayer.width >= 8;

    if !can_simd_interior {
        // Fall back to scalar for this row
        process_row_scalar(bayer, row_rgb, y);
        return;
    }

    unsafe {
        let half = _mm_set1_ps(0.5);
        let quarter = _mm_set1_ps(0.25);

        let row_above = (raw_y - 1) * bayer.raw_width;
        let row_current = raw_y * bayer.raw_width;
        let row_below = (raw_y + 1) * bayer.raw_width;

        // Process left border pixel(s) with scalar
        let left_border = 1.min(bayer.width);
        for x in 0..left_border {
            let raw_x = x + bayer.left_margin;
            let rgb_idx = x * 3;
            let color = pattern[row_pattern_idx | (raw_x & 1)];
            let val = bayer.data[raw_y * bayer.raw_width + raw_x];

            match color {
                0 => {
                    row_rgb[rgb_idx] = val;
                    row_rgb[rgb_idx + 1] = bayer::scalar::interpolate_cross(bayer, raw_x, raw_y);
                    row_rgb[rgb_idx + 2] = bayer::scalar::interpolate_diagonal(bayer, raw_x, raw_y);
                }
                1 => {
                    if red_in_row {
                        row_rgb[rgb_idx] =
                            bayer::scalar::interpolate_horizontal(bayer, raw_x, raw_y);
                        row_rgb[rgb_idx + 2] =
                            bayer::scalar::interpolate_vertical(bayer, raw_x, raw_y);
                    } else {
                        row_rgb[rgb_idx] = bayer::scalar::interpolate_vertical(bayer, raw_x, raw_y);
                        row_rgb[rgb_idx + 2] =
                            bayer::scalar::interpolate_horizontal(bayer, raw_x, raw_y);
                    }
                    row_rgb[rgb_idx + 1] = val;
                }
                2 => {
                    row_rgb[rgb_idx] = bayer::scalar::interpolate_diagonal(bayer, raw_x, raw_y);
                    row_rgb[rgb_idx + 1] = bayer::scalar::interpolate_cross(bayer, raw_x, raw_y);
                    row_rgb[rgb_idx + 2] = val;
                }
                _ => unreachable!(),
            }
        }

        // Process interior with SIMD (4 pixels at a time)
        let mut x = left_border;
        let simd_end = if bayer.width > 1 {
            bayer.width - 1
        } else {
            left_border
        };

        while x + 4 <= simd_end {
            let raw_x = x + bayer.left_margin;

            // Load 4 consecutive pixels and neighbors
            let center = _mm_loadu_ps(bayer.data.as_ptr().add(row_current + raw_x));
            let left = _mm_loadu_ps(bayer.data.as_ptr().add(row_current + raw_x - 1));
            let right = _mm_loadu_ps(bayer.data.as_ptr().add(row_current + raw_x + 1));
            let top = _mm_loadu_ps(bayer.data.as_ptr().add(row_above + raw_x));
            let bottom = _mm_loadu_ps(bayer.data.as_ptr().add(row_below + raw_x));

            // Diagonal neighbors
            let top_left = _mm_loadu_ps(bayer.data.as_ptr().add(row_above + raw_x - 1));
            let top_right = _mm_loadu_ps(bayer.data.as_ptr().add(row_above + raw_x + 1));
            let bottom_left = _mm_loadu_ps(bayer.data.as_ptr().add(row_below + raw_x - 1));
            let bottom_right = _mm_loadu_ps(bayer.data.as_ptr().add(row_below + raw_x + 1));

            // Compute all interpolations in parallel
            let h_interp = _mm_mul_ps(_mm_add_ps(left, right), half);
            let v_interp = _mm_mul_ps(_mm_add_ps(top, bottom), half);
            let cross_interp = _mm_mul_ps(
                _mm_add_ps(_mm_add_ps(left, right), _mm_add_ps(top, bottom)),
                quarter,
            );
            let diag_interp = _mm_mul_ps(
                _mm_add_ps(
                    _mm_add_ps(top_left, top_right),
                    _mm_add_ps(bottom_left, bottom_right),
                ),
                quarter,
            );

            // Extract to arrays for assignment
            let center_arr: [f32; 4] = std::mem::transmute(center);
            let h_arr: [f32; 4] = std::mem::transmute(h_interp);
            let v_arr: [f32; 4] = std::mem::transmute(v_interp);
            let cross_arr: [f32; 4] = std::mem::transmute(cross_interp);
            let diag_arr: [f32; 4] = std::mem::transmute(diag_interp);

            // Assign based on pattern (unrolled for efficiency)
            // The pattern repeats every 2 pixels, so we handle 2 pairs
            for i in 0..4 {
                let px = x + i;
                let raw_px = raw_x + i;
                let rgb_idx = px * 3;
                let color = pattern[row_pattern_idx | (raw_px & 1)];

                match color {
                    0 => {
                        row_rgb[rgb_idx] = center_arr[i];
                        row_rgb[rgb_idx + 1] = cross_arr[i];
                        row_rgb[rgb_idx + 2] = diag_arr[i];
                    }
                    1 => {
                        if red_in_row {
                            row_rgb[rgb_idx] = h_arr[i];
                            row_rgb[rgb_idx + 2] = v_arr[i];
                        } else {
                            row_rgb[rgb_idx] = v_arr[i];
                            row_rgb[rgb_idx + 2] = h_arr[i];
                        }
                        row_rgb[rgb_idx + 1] = center_arr[i];
                    }
                    2 => {
                        row_rgb[rgb_idx] = diag_arr[i];
                        row_rgb[rgb_idx + 1] = cross_arr[i];
                        row_rgb[rgb_idx + 2] = center_arr[i];
                    }
                    _ => unreachable!(),
                }
            }

            x += 4;
        }

        // Process remaining pixels with scalar
        while x < bayer.width {
            let raw_x = x + bayer.left_margin;
            let rgb_idx = x * 3;
            let color = pattern[row_pattern_idx | (raw_x & 1)];
            let val = bayer.data[raw_y * bayer.raw_width + raw_x];

            match color {
                0 => {
                    row_rgb[rgb_idx] = val;
                    row_rgb[rgb_idx + 1] = bayer::scalar::interpolate_cross(bayer, raw_x, raw_y);
                    row_rgb[rgb_idx + 2] = bayer::scalar::interpolate_diagonal(bayer, raw_x, raw_y);
                }
                1 => {
                    if red_in_row {
                        row_rgb[rgb_idx] =
                            bayer::scalar::interpolate_horizontal(bayer, raw_x, raw_y);
                        row_rgb[rgb_idx + 2] =
                            bayer::scalar::interpolate_vertical(bayer, raw_x, raw_y);
                    } else {
                        row_rgb[rgb_idx] = bayer::scalar::interpolate_vertical(bayer, raw_x, raw_y);
                        row_rgb[rgb_idx + 2] =
                            bayer::scalar::interpolate_horizontal(bayer, raw_x, raw_y);
                    }
                    row_rgb[rgb_idx + 1] = val;
                }
                2 => {
                    row_rgb[rgb_idx] = bayer::scalar::interpolate_diagonal(bayer, raw_x, raw_y);
                    row_rgb[rgb_idx + 1] = bayer::scalar::interpolate_cross(bayer, raw_x, raw_y);
                    row_rgb[rgb_idx + 2] = val;
                }
                _ => unreachable!(),
            }
            x += 1;
        }
    }
}

/// Process a single row with NEON SIMD.
#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
unsafe fn process_row_simd_neon(bayer: &BayerImage, row_rgb: &mut [f32], y: usize) {
    use std::arch::aarch64::*;

    let raw_y = y + bayer.top_margin;
    let pattern = bayer.cfa.pattern_2x2();
    let red_in_row = bayer.cfa.red_in_row(raw_y);
    let row_pattern_idx = (raw_y & 1) << 1;

    // Check if we have enough margin for safe SIMD access
    let can_simd_interior =
        raw_y > 0 && raw_y + 1 < bayer.raw_height && bayer.left_margin > 0 && bayer.width >= 8;

    if !can_simd_interior {
        process_row_scalar(bayer, row_rgb, y);
        return;
    }

    unsafe {
        let half = vdupq_n_f32(0.5);
        let quarter = vdupq_n_f32(0.25);

        let row_above = (raw_y - 1) * bayer.raw_width;
        let row_current = raw_y * bayer.raw_width;
        let row_below = (raw_y + 1) * bayer.raw_width;

        // Process left border pixel(s) with scalar
        let left_border = 1.min(bayer.width);
        for x in 0..left_border {
            let raw_x = x + bayer.left_margin;
            let rgb_idx = x * 3;
            let color = pattern[row_pattern_idx | (raw_x & 1)];
            let val = bayer.data[raw_y * bayer.raw_width + raw_x];

            match color {
                0 => {
                    row_rgb[rgb_idx] = val;
                    row_rgb[rgb_idx + 1] = bayer::scalar::interpolate_cross(bayer, raw_x, raw_y);
                    row_rgb[rgb_idx + 2] = bayer::scalar::interpolate_diagonal(bayer, raw_x, raw_y);
                }
                1 => {
                    if red_in_row {
                        row_rgb[rgb_idx] =
                            bayer::scalar::interpolate_horizontal(bayer, raw_x, raw_y);
                        row_rgb[rgb_idx + 2] =
                            bayer::scalar::interpolate_vertical(bayer, raw_x, raw_y);
                    } else {
                        row_rgb[rgb_idx] = bayer::scalar::interpolate_vertical(bayer, raw_x, raw_y);
                        row_rgb[rgb_idx + 2] =
                            bayer::scalar::interpolate_horizontal(bayer, raw_x, raw_y);
                    }
                    row_rgb[rgb_idx + 1] = val;
                }
                2 => {
                    row_rgb[rgb_idx] = bayer::scalar::interpolate_diagonal(bayer, raw_x, raw_y);
                    row_rgb[rgb_idx + 1] = bayer::scalar::interpolate_cross(bayer, raw_x, raw_y);
                    row_rgb[rgb_idx + 2] = val;
                }
                _ => unreachable!(),
            }
        }

        // Process interior with SIMD
        let mut x = left_border;
        let simd_end = if bayer.width > 1 {
            bayer.width - 1
        } else {
            left_border
        };

        while x + 4 <= simd_end {
            let raw_x = x + bayer.left_margin;

            // Load 4 consecutive pixels and neighbors
            let center = vld1q_f32(bayer.data.as_ptr().add(row_current + raw_x));
            let left = vld1q_f32(bayer.data.as_ptr().add(row_current + raw_x - 1));
            let right = vld1q_f32(bayer.data.as_ptr().add(row_current + raw_x + 1));
            let top = vld1q_f32(bayer.data.as_ptr().add(row_above + raw_x));
            let bottom = vld1q_f32(bayer.data.as_ptr().add(row_below + raw_x));

            // Diagonal neighbors
            let top_left = vld1q_f32(bayer.data.as_ptr().add(row_above + raw_x - 1));
            let top_right = vld1q_f32(bayer.data.as_ptr().add(row_above + raw_x + 1));
            let bottom_left = vld1q_f32(bayer.data.as_ptr().add(row_below + raw_x - 1));
            let bottom_right = vld1q_f32(bayer.data.as_ptr().add(row_below + raw_x + 1));

            // Compute all interpolations in parallel
            let h_interp = vmulq_f32(vaddq_f32(left, right), half);
            let v_interp = vmulq_f32(vaddq_f32(top, bottom), half);
            let cross_interp = vmulq_f32(
                vaddq_f32(vaddq_f32(left, right), vaddq_f32(top, bottom)),
                quarter,
            );
            let diag_interp = vmulq_f32(
                vaddq_f32(
                    vaddq_f32(top_left, top_right),
                    vaddq_f32(bottom_left, bottom_right),
                ),
                quarter,
            );

            // Extract to arrays
            let center_arr: [f32; 4] = std::mem::transmute(center);
            let h_arr: [f32; 4] = std::mem::transmute(h_interp);
            let v_arr: [f32; 4] = std::mem::transmute(v_interp);
            let cross_arr: [f32; 4] = std::mem::transmute(cross_interp);
            let diag_arr: [f32; 4] = std::mem::transmute(diag_interp);

            for i in 0..4 {
                let px = x + i;
                let raw_px = raw_x + i;
                let rgb_idx = px * 3;
                let color = pattern[row_pattern_idx | (raw_px & 1)];

                match color {
                    0 => {
                        row_rgb[rgb_idx] = center_arr[i];
                        row_rgb[rgb_idx + 1] = cross_arr[i];
                        row_rgb[rgb_idx + 2] = diag_arr[i];
                    }
                    1 => {
                        if red_in_row {
                            row_rgb[rgb_idx] = h_arr[i];
                            row_rgb[rgb_idx + 2] = v_arr[i];
                        } else {
                            row_rgb[rgb_idx] = v_arr[i];
                            row_rgb[rgb_idx + 2] = h_arr[i];
                        }
                        row_rgb[rgb_idx + 1] = center_arr[i];
                    }
                    2 => {
                        row_rgb[rgb_idx] = diag_arr[i];
                        row_rgb[rgb_idx + 1] = cross_arr[i];
                        row_rgb[rgb_idx + 2] = center_arr[i];
                    }
                    _ => unreachable!(),
                }
            }

            x += 4;
        }

        // Process remaining pixels with scalar
        while x < bayer.width {
            let raw_x = x + bayer.left_margin;
            let rgb_idx = x * 3;
            let color = pattern[row_pattern_idx | (raw_x & 1)];
            let val = bayer.data[raw_y * bayer.raw_width + raw_x];

            match color {
                0 => {
                    row_rgb[rgb_idx] = val;
                    row_rgb[rgb_idx + 1] = bayer::scalar::interpolate_cross(bayer, raw_x, raw_y);
                    row_rgb[rgb_idx + 2] = bayer::scalar::interpolate_diagonal(bayer, raw_x, raw_y);
                }
                1 => {
                    if red_in_row {
                        row_rgb[rgb_idx] =
                            bayer::scalar::interpolate_horizontal(bayer, raw_x, raw_y);
                        row_rgb[rgb_idx + 2] =
                            bayer::scalar::interpolate_vertical(bayer, raw_x, raw_y);
                    } else {
                        row_rgb[rgb_idx] = bayer::scalar::interpolate_vertical(bayer, raw_x, raw_y);
                        row_rgb[rgb_idx + 2] =
                            bayer::scalar::interpolate_horizontal(bayer, raw_x, raw_y);
                    }
                    row_rgb[rgb_idx + 1] = val;
                }
                2 => {
                    row_rgb[rgb_idx] = bayer::scalar::interpolate_diagonal(bayer, raw_x, raw_y);
                    row_rgb[rgb_idx + 1] = bayer::scalar::interpolate_cross(bayer, raw_x, raw_y);
                    row_rgb[rgb_idx + 2] = val;
                }
                _ => unreachable!(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Test CFA patterns
    #[test]
    fn test_cfa_rggb_pattern() {
        let cfa = CfaPattern::Rggb;
        // Row 0: R G R G ...
        assert_eq!(cfa.color_at(0, 0), 0); // R
        assert_eq!(cfa.color_at(0, 1), 1); // G
        assert_eq!(cfa.color_at(0, 2), 0); // R
        assert_eq!(cfa.color_at(0, 3), 1); // G
        // Row 1: G B G B ...
        assert_eq!(cfa.color_at(1, 0), 1); // G
        assert_eq!(cfa.color_at(1, 1), 2); // B
        assert_eq!(cfa.color_at(1, 2), 1); // G
        assert_eq!(cfa.color_at(1, 3), 2); // B
    }

    #[test]
    fn test_cfa_bggr_pattern() {
        let cfa = CfaPattern::Bggr;
        // Row 0: B G B G ...
        assert_eq!(cfa.color_at(0, 0), 2); // B
        assert_eq!(cfa.color_at(0, 1), 1); // G
        // Row 1: G R G R ...
        assert_eq!(cfa.color_at(1, 0), 1); // G
        assert_eq!(cfa.color_at(1, 1), 0); // R
    }

    #[test]
    fn test_cfa_grbg_pattern() {
        let cfa = CfaPattern::Grbg;
        // Row 0: G R G R ...
        assert_eq!(cfa.color_at(0, 0), 1); // G
        assert_eq!(cfa.color_at(0, 1), 0); // R
        // Row 1: B G B G ...
        assert_eq!(cfa.color_at(1, 0), 2); // B
        assert_eq!(cfa.color_at(1, 1), 1); // G
    }

    #[test]
    fn test_cfa_gbrg_pattern() {
        let cfa = CfaPattern::Gbrg;
        // Row 0: G B G B ...
        assert_eq!(cfa.color_at(0, 0), 1); // G
        assert_eq!(cfa.color_at(0, 1), 2); // B
        // Row 1: R G R G ...
        assert_eq!(cfa.color_at(1, 0), 0); // R
        assert_eq!(cfa.color_at(1, 1), 1); // G
    }

    #[test]
    fn test_red_in_row() {
        // RGGB: Red in row 0, 2, 4, ...
        assert!(CfaPattern::Rggb.red_in_row(0));
        assert!(!CfaPattern::Rggb.red_in_row(1));
        assert!(CfaPattern::Rggb.red_in_row(2));

        // BGGR: Red in row 1, 3, 5, ...
        assert!(!CfaPattern::Bggr.red_in_row(0));
        assert!(CfaPattern::Bggr.red_in_row(1));
        assert!(!CfaPattern::Bggr.red_in_row(2));

        // GRBG: Red in row 0, 2, 4, ...
        assert!(CfaPattern::Grbg.red_in_row(0));
        assert!(!CfaPattern::Grbg.red_in_row(1));

        // GBRG: Red in row 1, 3, 5, ...
        assert!(!CfaPattern::Gbrg.red_in_row(0));
        assert!(CfaPattern::Gbrg.red_in_row(1));
    }

    #[test]
    fn test_pattern_2x2() {
        assert_eq!(CfaPattern::Rggb.pattern_2x2(), [0, 1, 1, 2]);
        assert_eq!(CfaPattern::Bggr.pattern_2x2(), [2, 1, 1, 0]);
        assert_eq!(CfaPattern::Grbg.pattern_2x2(), [1, 0, 2, 1]);
        assert_eq!(CfaPattern::Gbrg.pattern_2x2(), [1, 2, 0, 1]);
    }

    // Test BayerImage validation
    #[test]
    #[should_panic(expected = "Output dimensions must be non-zero")]
    fn test_bayer_image_zero_width() {
        let data = vec![0.0f32; 4];
        BayerImage::with_margins(&data, 2, 2, 0, 2, 0, 0, CfaPattern::Rggb);
    }

    #[test]
    #[should_panic(expected = "Output dimensions must be non-zero")]
    fn test_bayer_image_zero_height() {
        let data = vec![0.0f32; 4];
        BayerImage::with_margins(&data, 2, 2, 2, 0, 0, 0, CfaPattern::Rggb);
    }

    #[test]
    #[should_panic(expected = "Data length")]
    fn test_bayer_image_wrong_data_length() {
        let data = vec![0.0f32; 3]; // Should be 4
        BayerImage::with_margins(&data, 2, 2, 2, 2, 0, 0, CfaPattern::Rggb);
    }

    #[test]
    #[should_panic(expected = "Top margin")]
    fn test_bayer_image_margin_exceeds_height() {
        let data = vec![0.0f32; 4];
        BayerImage::with_margins(&data, 2, 2, 2, 2, 1, 0, CfaPattern::Rggb);
    }

    #[test]
    #[should_panic(expected = "Left margin")]
    fn test_bayer_image_margin_exceeds_width() {
        let data = vec![0.0f32; 4];
        BayerImage::with_margins(&data, 2, 2, 2, 2, 0, 1, CfaPattern::Rggb);
    }

    #[test]
    fn test_bayer_image_valid() {
        let data = vec![0.0f32; 16];
        let bayer = BayerImage::with_margins(&data, 4, 4, 2, 2, 1, 1, CfaPattern::Rggb);
        assert_eq!(bayer.raw_width, 4);
        assert_eq!(bayer.raw_height, 4);
        assert_eq!(bayer.width, 2);
        assert_eq!(bayer.height, 2);
        assert_eq!(bayer.top_margin, 1);
        assert_eq!(bayer.left_margin, 1);
    }

    // Test demosaicing
    #[test]
    fn test_demosaic_output_size() {
        // 4x4 Bayer -> 4x4x3 RGB
        let data = vec![0.5f32; 16];
        let bayer = BayerImage::with_margins(&data, 4, 4, 4, 4, 0, 0, CfaPattern::Rggb);
        let rgb = demosaic_bilinear(&bayer);
        assert_eq!(rgb.len(), 4 * 4 * 3);
    }

    #[test]
    fn test_demosaic_with_margins() {
        // 6x6 raw with 1px margin -> 4x4 output
        let data = vec![0.5f32; 36];
        let bayer = BayerImage::with_margins(&data, 6, 6, 4, 4, 1, 1, CfaPattern::Rggb);
        let rgb = demosaic_bilinear(&bayer);
        assert_eq!(rgb.len(), 4 * 4 * 3);
    }

    #[test]
    fn test_demosaic_uniform_gray() {
        // Uniform gray input should produce uniform gray output
        let data = vec![0.5f32; 16];
        let bayer = BayerImage::with_margins(&data, 4, 4, 4, 4, 0, 0, CfaPattern::Rggb);
        let rgb = demosaic_bilinear(&bayer);

        for &v in &rgb {
            assert!((v - 0.5).abs() < 0.01, "Expected ~0.5, got {}", v);
        }
    }

    #[test]
    fn test_demosaic_preserves_red_at_red_pixel() {
        // Create a pattern where red pixels have value 1.0, others 0.0
        // RGGB pattern: (0,0) is red
        let mut data = vec![0.0f32; 16];
        // Set red pixels (0,0), (0,2), (2,0), (2,2) to 1.0
        data[0] = 1.0; // (0,0)
        data[2] = 1.0; // (0,2)
        data[8] = 1.0; // (2,0)
        data[10] = 1.0; // (2,2)

        let bayer = BayerImage::with_margins(&data, 4, 4, 4, 4, 0, 0, CfaPattern::Rggb);
        let rgb = demosaic_bilinear(&bayer);

        // At (0,0), R channel should be 1.0
        assert!((rgb[0] - 1.0).abs() < 0.01, "Red at (0,0) should be 1.0");
        // G and B at (0,0) should be 0.0 (interpolated from neighbors)
        assert!(rgb[1].abs() < 0.01, "Green at (0,0) should be ~0.0");
        assert!(rgb[2].abs() < 0.01, "Blue at (0,0) should be ~0.0");
    }

    #[test]
    fn test_demosaic_2x2_rggb() {
        // Minimal 2x2 RGGB pattern
        // R=1.0, G1=0.5, G2=0.5, B=0.0
        let data = vec![1.0, 0.5, 0.5, 0.0];
        let bayer = BayerImage::with_margins(&data, 2, 2, 2, 2, 0, 0, CfaPattern::Rggb);
        let rgb = demosaic_bilinear(&bayer);

        assert_eq!(rgb.len(), 12); // 2x2x3

        // (0,0) is R: R=1.0, G=interpolated, B=interpolated
        assert!((rgb[0] - 1.0).abs() < 0.01);

        // (0,1) is G: G=0.5
        assert!((rgb[4] - 0.5).abs() < 0.01);

        // (1,1) is B: B=0.0
        assert!((rgb[11] - 0.0).abs() < 0.01);
    }

    #[test]
    fn test_demosaic_edge_interpolation() {
        // Test that edge pixels don't panic and produce reasonable values
        let data = vec![0.5f32; 4]; // 2x2 minimum
        let bayer = BayerImage::with_margins(&data, 2, 2, 2, 2, 0, 0, CfaPattern::Rggb);
        let rgb = demosaic_bilinear(&bayer);

        // All values should be around 0.5 since input is uniform
        for &v in &rgb {
            assert!((0.0..=1.0).contains(&v), "Value {} out of range", v);
        }
    }

    #[test]
    fn test_demosaic_all_cfa_patterns() {
        // Test all CFA patterns produce valid output
        let data = vec![0.5f32; 16];

        for cfa in [
            CfaPattern::Rggb,
            CfaPattern::Bggr,
            CfaPattern::Grbg,
            CfaPattern::Gbrg,
        ] {
            let bayer = BayerImage::with_margins(&data, 4, 4, 4, 4, 0, 0, cfa);
            let rgb = demosaic_bilinear(&bayer);
            assert_eq!(rgb.len(), 48);
            for &v in &rgb {
                assert!(
                    (v - 0.5).abs() < 0.01,
                    "CFA {:?}: expected ~0.5, got {}",
                    cfa,
                    v
                );
            }
        }
    }

    #[test]
    fn test_demosaic_simd_vs_scalar_consistency() {
        // Test that SIMD and scalar produce identical results for larger images
        let data: Vec<f32> = (0..100).map(|i| (i as f32 % 10.0) / 10.0).collect();
        let bayer = BayerImage::with_margins(&data, 10, 10, 10, 10, 0, 0, CfaPattern::Rggb);

        let rgb_main = demosaic_bilinear(&bayer);
        let rgb_scalar = bayer::scalar::demosaic_bilinear_scalar(&bayer);

        assert_eq!(rgb_main.len(), rgb_scalar.len());
        for (i, (&a, &b)) in rgb_main.iter().zip(rgb_scalar.iter()).enumerate() {
            assert!(
                (a - b).abs() < 1e-5,
                "Mismatch at index {}: SIMD={}, scalar={}",
                i,
                a,
                b
            );
        }
    }

    #[test]
    fn test_demosaic_large_image() {
        // Test with a larger image that exercises SIMD paths
        let size = 64;
        let data: Vec<f32> = (0..size * size)
            .map(|i| (i as f32 % 256.0) / 255.0)
            .collect();
        let bayer = BayerImage::with_margins(&data, size, size, size, size, 0, 0, CfaPattern::Rggb);

        let rgb = demosaic_bilinear(&bayer);
        assert_eq!(rgb.len(), size * size * 3);

        // All values should be in valid range
        for &v in &rgb {
            assert!(
                (0.0..=1.5).contains(&v),
                "Value {} out of expected range",
                v
            );
        }
    }

    #[test]
    fn test_demosaic_all_cfa_large() {
        // Test all CFA patterns with larger images to exercise SIMD
        let size = 32;
        let data: Vec<f32> = (0..size * size)
            .map(|i| (i as f32 % 100.0) / 100.0)
            .collect();

        for cfa in [
            CfaPattern::Rggb,
            CfaPattern::Bggr,
            CfaPattern::Grbg,
            CfaPattern::Gbrg,
        ] {
            let bayer = BayerImage::with_margins(&data, size, size, size, size, 0, 0, cfa);

            let rgb_main = demosaic_bilinear(&bayer);
            let rgb_scalar = bayer::scalar::demosaic_bilinear_scalar(&bayer);

            for (i, (&a, &b)) in rgb_main.iter().zip(rgb_scalar.iter()).enumerate() {
                assert!(
                    (a - b).abs() < 1e-5,
                    "CFA {:?}: Mismatch at index {}: SIMD={}, scalar={}",
                    cfa,
                    i,
                    a,
                    b
                );
            }
        }
    }

    #[test]
    fn test_demosaic_parallel_vs_scalar() {
        // Test that parallel processing produces same results as scalar
        let size = 256; // Large enough to trigger parallel processing
        let data: Vec<f32> = (0..size * size)
            .map(|i| (i as f32 % 100.0) / 100.0)
            .collect();

        for cfa in [
            CfaPattern::Rggb,
            CfaPattern::Bggr,
            CfaPattern::Grbg,
            CfaPattern::Gbrg,
        ] {
            let bayer = BayerImage::with_margins(&data, size, size, size, size, 0, 0, cfa);

            let rgb_parallel = demosaic_bilinear(&bayer);
            let rgb_scalar = bayer::scalar::demosaic_bilinear_scalar(&bayer);

            assert_eq!(rgb_parallel.len(), rgb_scalar.len());
            for (i, (&a, &b)) in rgb_parallel.iter().zip(rgb_scalar.iter()).enumerate() {
                assert!(
                    (a - b).abs() < 1e-5,
                    "CFA {:?}: Mismatch at index {}: parallel={}, scalar={}",
                    cfa,
                    i,
                    a,
                    b
                );
            }
        }
    }

    #[test]
    fn test_interpolate_horizontal_edge_cases() {
        let data = vec![0.0, 1.0, 0.0, 1.0];
        let bayer = BayerImage::with_margins(&data, 2, 2, 2, 2, 0, 0, CfaPattern::Rggb);

        // At x=0, should use right neighbor twice
        let h0 = bayer::scalar::interpolate_horizontal(&bayer, 0, 0);
        assert!((h0 - 1.0).abs() < 0.01); // (data[1] + data[1]) / 2

        // At x=1, should use left neighbor twice
        let h1 = bayer::scalar::interpolate_horizontal(&bayer, 1, 0);
        assert!((h1 - 0.0).abs() < 0.01); // (data[0] + data[0]) / 2
    }

    #[test]
    fn test_interpolate_vertical_edge_cases() {
        let data = vec![0.0, 0.0, 1.0, 1.0];
        let bayer = BayerImage::with_margins(&data, 2, 2, 2, 2, 0, 0, CfaPattern::Rggb);

        // At y=0, should use bottom neighbor twice
        let v0 = bayer::scalar::interpolate_vertical(&bayer, 0, 0);
        assert!((v0 - 1.0).abs() < 0.01); // (data[2] + data[2]) / 2

        // At y=1, should use top neighbor twice
        let v1 = bayer::scalar::interpolate_vertical(&bayer, 0, 1);
        assert!((v1 - 0.0).abs() < 0.01); // (data[0] + data[0]) / 2
    }
}

#[cfg(feature = "bench")]
pub mod bench;
