//! ARM aarch64 NEON SIMD implementation of X-Trans bilinear demosaicing.
//!
//! This implementation processes 4 pixels at a time using NEON intrinsics.
//! For each pixel, it loads neighboring pixels and computes the interpolated
//! RGB values in parallel.

use super::XTransImage;
use super::scalar::NeighborLookup;

/// Minimum image width to use SIMD (need enough pixels for vectorized loads).
const MIN_SIMD_WIDTH: usize = 8;

/// Search radius for interpolation (5x5 neighborhood).
const SEARCH_RADIUS: usize = 2;

/// Process a row of X-Trans image data using NEON SIMD.
///
/// # Safety
/// Caller must ensure NEON is available on the target CPU.
#[target_feature(enable = "neon")]
pub(crate) unsafe fn process_row_simd_neon(
    xtrans: &XTransImage,
    y: usize,
    row_rgb: &mut [f32],
    lookups: &[&NeighborLookup; 3],
) {
    let raw_y = y + xtrans.top_margin;
    let row_base = raw_y * xtrans.raw_width;

    // Check if this row is in the interior (no vertical bounds checking needed)
    let is_interior_y = raw_y >= SEARCH_RADIUS && raw_y + SEARCH_RADIUS < xtrans.raw_height;

    // Determine SIMD-safe range
    let simd_start = SEARCH_RADIUS.saturating_sub(xtrans.left_margin);

    let simd_end = if xtrans.left_margin + xtrans.width + SEARCH_RADIUS <= xtrans.raw_width {
        xtrans.width
    } else {
        xtrans
            .raw_width
            .saturating_sub(xtrans.left_margin + SEARCH_RADIUS)
    };

    // Process left edge pixels with scalar
    for x in 0..simd_start.min(xtrans.width) {
        process_pixel_scalar(xtrans, x, raw_y, row_base, row_rgb, lookups, false);
    }

    // Process interior with SIMD if row is interior and we have enough width
    if is_interior_y && simd_end > simd_start && xtrans.width >= MIN_SIMD_WIDTH {
        let mut x = simd_start;

        // Process 4 pixels at a time
        while x + 4 <= simd_end {
            // SAFETY: We're inside an unsafe fn with neon target feature,
            // and we've verified this is an interior region
            unsafe {
                process_4_pixels_simd(xtrans, x, raw_y, row_rgb, lookups);
            }
            x += 4;
        }

        // Process remaining interior pixels with scalar (fast path)
        while x < simd_end {
            process_pixel_scalar(xtrans, x, raw_y, row_base, row_rgb, lookups, true);
            x += 1;
        }

        // Process right edge pixels with scalar (safe path)
        for x in simd_end..xtrans.width {
            process_pixel_scalar(xtrans, x, raw_y, row_base, row_rgb, lookups, false);
        }
    } else {
        // Fall back to scalar for entire row
        for x in simd_start.min(xtrans.width)..xtrans.width {
            let is_interior = is_interior_y
                && (x + xtrans.left_margin) >= SEARCH_RADIUS
                && (x + xtrans.left_margin) + SEARCH_RADIUS < xtrans.raw_width;
            process_pixel_scalar(xtrans, x, raw_y, row_base, row_rgb, lookups, is_interior);
        }
    }
}

/// Process 4 consecutive pixels using NEON SIMD.
///
/// # Safety
/// - Caller must ensure NEON is available
/// - Pixels must be in interior (no bounds checking)
#[target_feature(enable = "neon")]
#[inline]
unsafe fn process_4_pixels_simd(
    xtrans: &XTransImage,
    x: usize,
    raw_y: usize,
    row_rgb: &mut [f32],
    lookups: &[&NeighborLookup; 3],
) {
    use std::arch::aarch64::*;

    // Pre-compute raw positions
    let raw_x_base = x + xtrans.left_margin;

    // Load 4 center pixel values
    let row_base = raw_y * xtrans.raw_width;

    // SAFETY: We're in an unsafe block, neon is enabled, and we've verified bounds
    let center = unsafe { vld1q_f32(xtrans.data.as_ptr().add(row_base + raw_x_base)) };

    // Interpolate each color channel for all 4 pixels
    let red_interp = unsafe { interpolate_4_pixels(xtrans, raw_x_base, raw_y, lookups[0]) };
    let green_interp = unsafe { interpolate_4_pixels(xtrans, raw_x_base, raw_y, lookups[1]) };
    let blue_interp = unsafe { interpolate_4_pixels(xtrans, raw_x_base, raw_y, lookups[2]) };

    // Extract values to arrays for assignment
    // SAFETY: float32x4_t and [f32; 4] have the same memory layout
    let center_arr: [f32; 4] = unsafe { std::mem::transmute(center) };
    let red_arr: [f32; 4] = unsafe { std::mem::transmute(red_interp) };
    let green_arr: [f32; 4] = unsafe { std::mem::transmute(green_interp) };
    let blue_arr: [f32; 4] = unsafe { std::mem::transmute(blue_interp) };

    // Assign based on pattern - use known value for pixel's color, interpolated for others
    for i in 0..4 {
        let px = x + i;
        let raw_px = raw_x_base + i;
        let rgb_idx = px * 3;
        let color = xtrans.pattern.color_at(raw_y, raw_px);

        match color {
            0 => {
                // Red pixel - use center for red, interpolated for green/blue
                row_rgb[rgb_idx] = center_arr[i];
                row_rgb[rgb_idx + 1] = green_arr[i];
                row_rgb[rgb_idx + 2] = blue_arr[i];
            }
            1 => {
                // Green pixel - use center for green, interpolated for red/blue
                row_rgb[rgb_idx] = red_arr[i];
                row_rgb[rgb_idx + 1] = center_arr[i];
                row_rgb[rgb_idx + 2] = blue_arr[i];
            }
            2 => {
                // Blue pixel - use center for blue, interpolated for red/green
                row_rgb[rgb_idx] = red_arr[i];
                row_rgb[rgb_idx + 1] = green_arr[i];
                row_rgb[rgb_idx + 2] = center_arr[i];
            }
            _ => unreachable!(),
        }
    }
}

/// Interpolate a color channel for 4 consecutive pixels using SIMD.
///
/// Returns a vector of 4 interpolated values.
#[target_feature(enable = "neon")]
#[inline]
unsafe fn interpolate_4_pixels(
    xtrans: &XTransImage,
    raw_x_base: usize,
    raw_y: usize,
    lookup: &NeighborLookup,
) -> std::arch::aarch64::float32x4_t {
    use std::arch::aarch64::*;

    // For each of the 4 pixels, accumulate neighbor values
    let mut sums = [0.0f32; 4];

    for (i, sum_out) in sums.iter_mut().enumerate() {
        let raw_x = raw_x_base + i;
        let offset_list = lookup.get(raw_y, raw_x);
        let offsets = offset_list.as_slice();

        let mut sum = 0.0f32;
        for &(dy, dx) in offsets {
            let ny = (raw_y as i32 + dy) as usize;
            let nx = (raw_x as i32 + dx) as usize;
            sum += xtrans.data[ny * xtrans.raw_width + nx];
        }
        *sum_out = sum * offset_list.inv_len();
    }

    // SAFETY: neon is enabled
    unsafe { vld1q_f32(sums.as_ptr()) }
}

/// Process a single pixel with scalar code.
#[inline]
fn process_pixel_scalar(
    xtrans: &XTransImage,
    x: usize,
    raw_y: usize,
    row_base: usize,
    row_rgb: &mut [f32],
    lookups: &[&NeighborLookup; 3],
    is_interior: bool,
) {
    let raw_x = x + xtrans.left_margin;
    let rgb_idx = x * 3;
    let color = xtrans.pattern.color_at(raw_y, raw_x);
    let val = xtrans.data[row_base + raw_x];

    // Set the known color channel
    row_rgb[rgb_idx + color as usize] = val;

    // Interpolate the other two channels
    if is_interior {
        for c in 0u8..3 {
            if c != color {
                row_rgb[rgb_idx + c as usize] =
                    interpolate_channel_fast(xtrans, raw_x, raw_y, lookups[c as usize]);
            }
        }
    } else {
        for c in 0u8..3 {
            if c != color {
                row_rgb[rgb_idx + c as usize] =
                    interpolate_channel_safe(xtrans, raw_x, raw_y, lookups[c as usize]);
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
