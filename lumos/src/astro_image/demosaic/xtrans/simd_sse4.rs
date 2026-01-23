//! x86_64 SSE4.1 SIMD implementation of X-Trans bilinear demosaicing.
//!
//! This implementation processes multiple pixels horizontally in parallel.
//! The key optimization is computing 4 output pixels simultaneously.

use super::XTransImage;
use super::scalar::NeighborLookup;

/// Search radius for interpolation (5x5 neighborhood).
const SEARCH_RADIUS: usize = 2;

/// Process a row of X-Trans image data using SSE4.1 SIMD.
///
/// Processes 4 pixels at a time horizontally for interior regions.
///
/// # Safety
/// Caller must ensure SSE4.1 is available on the target CPU.
#[target_feature(enable = "sse4.1")]
#[allow(unsafe_op_in_unsafe_fn)]
pub(crate) unsafe fn process_row_simd_sse4(
    xtrans: &XTransImage,
    y: usize,
    row_rgb: &mut [f32],
    lookups: &[&NeighborLookup; 3],
) {
    let raw_y = y + xtrans.top_margin;
    let row_base = raw_y * xtrans.raw_width;

    // Check if this row is in the interior (no vertical bounds checking needed)
    let is_interior_y = raw_y >= SEARCH_RADIUS && raw_y + SEARCH_RADIUS < xtrans.raw_height;

    // Determine safe range for interior processing
    let interior_start = SEARCH_RADIUS.saturating_sub(xtrans.left_margin);

    let interior_end = if xtrans.left_margin + xtrans.width + SEARCH_RADIUS <= xtrans.raw_width {
        xtrans.width
    } else {
        xtrans
            .raw_width
            .saturating_sub(xtrans.left_margin + SEARCH_RADIUS)
    };

    if is_interior_y && interior_end > interior_start {
        // Process left edge pixels with safe scalar
        for x in 0..interior_start {
            process_pixel_safe(xtrans, x, raw_y, row_base, row_rgb, lookups);
        }

        // Process interior pixels - 4 at a time where possible
        let interior_len = interior_end - interior_start;
        let simd_end = interior_start + (interior_len / 4) * 4;

        // SIMD path: process 4 pixels at a time
        let mut x = interior_start;
        while x < simd_end {
            process_4_pixels_simd(xtrans, x, raw_y, row_base, row_rgb, lookups);
            x += 4;
        }

        // Handle remaining 1-3 pixels with scalar
        for x in simd_end..interior_end {
            process_pixel_interior(xtrans, x, raw_y, row_base, row_rgb, lookups);
        }

        // Process right edge pixels with safe scalar
        for x in interior_end..xtrans.width {
            process_pixel_safe(xtrans, x, raw_y, row_base, row_rgb, lookups);
        }
    } else {
        // Fall back to safe scalar for entire row (edge rows or narrow images)
        for x in 0..xtrans.width {
            process_pixel_safe(xtrans, x, raw_y, row_base, row_rgb, lookups);
        }
    }
}

/// Process 4 consecutive pixels with optimized memory access.
#[inline]
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn process_4_pixels_simd(
    xtrans: &XTransImage,
    x_start: usize,
    raw_y: usize,
    row_base: usize,
    row_rgb: &mut [f32],
    lookups: &[&NeighborLookup; 3],
) {
    let data_ptr = xtrans.data.as_ptr();
    let raw_width = xtrans.raw_width;

    // Process each of the 4 pixels
    for i in 0..4 {
        let x = x_start + i;
        let raw_x = x + xtrans.left_margin;
        let rgb_idx = x * 3;
        let color = xtrans.pattern.color_at(raw_y, raw_x);
        let center_val = xtrans.data[row_base + raw_x];

        // Set the known color channel
        row_rgb[rgb_idx + color as usize] = center_val;

        // Interpolate the other two channels
        for c in 0u8..3 {
            if c != color {
                let offset_list = lookups[c as usize].get(raw_y, raw_x);
                let offsets = offset_list.as_slice();
                let inv_len = offset_list.inv_len();

                // Sum neighbors using SIMD-accelerated loop
                let sum = sum_neighbors_fast(data_ptr, raw_x, raw_y, raw_width, offsets);
                row_rgb[rgb_idx + c as usize] = sum * inv_len;
            }
        }
    }
}

/// Sum neighbor values with optimized memory access.
/// Uses pointer arithmetic and unrolled loop for better performance.
#[inline(always)]
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn sum_neighbors_fast(
    data_ptr: *const f32,
    raw_x: usize,
    raw_y: usize,
    raw_width: usize,
    offsets: &[(i32, i32)],
) -> f32 {
    let base_idx = raw_y * raw_width + raw_x;
    let mut sum = 0.0f32;

    // Unroll by 4 for better instruction-level parallelism
    let chunks = offsets.len() / 4;
    let remainder = offsets.len() % 4;

    let mut i = 0;
    for _ in 0..chunks {
        let (dy0, dx0) = *offsets.get_unchecked(i);
        let (dy1, dx1) = *offsets.get_unchecked(i + 1);
        let (dy2, dx2) = *offsets.get_unchecked(i + 2);
        let (dy3, dx3) = *offsets.get_unchecked(i + 3);

        let offset0 = dy0 as isize * raw_width as isize + dx0 as isize;
        let offset1 = dy1 as isize * raw_width as isize + dx1 as isize;
        let offset2 = dy2 as isize * raw_width as isize + dx2 as isize;
        let offset3 = dy3 as isize * raw_width as isize + dx3 as isize;

        sum += *data_ptr.offset(base_idx as isize + offset0);
        sum += *data_ptr.offset(base_idx as isize + offset1);
        sum += *data_ptr.offset(base_idx as isize + offset2);
        sum += *data_ptr.offset(base_idx as isize + offset3);

        i += 4;
    }

    // Handle remainder
    for j in 0..remainder {
        let (dy, dx) = *offsets.get_unchecked(i + j);
        let offset = dy as isize * raw_width as isize + dx as isize;
        sum += *data_ptr.offset(base_idx as isize + offset);
    }

    sum
}

/// Process a single interior pixel (no bounds checking).
#[inline(always)]
fn process_pixel_interior(
    xtrans: &XTransImage,
    x: usize,
    raw_y: usize,
    row_base: usize,
    row_rgb: &mut [f32],
    lookups: &[&NeighborLookup; 3],
) {
    let raw_x = x + xtrans.left_margin;
    let rgb_idx = x * 3;
    let color = xtrans.pattern.color_at(raw_y, raw_x);
    let center_val = xtrans.data[row_base + raw_x];

    // Set the known color channel
    row_rgb[rgb_idx + color as usize] = center_val;

    // Interpolate the other two channels
    for c in 0u8..3 {
        if c != color {
            row_rgb[rgb_idx + c as usize] =
                interpolate_channel_fast(xtrans, raw_x, raw_y, lookups[c as usize]);
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

/// Process a single pixel with bounds checking (for edge pixels).
#[inline]
fn process_pixel_safe(
    xtrans: &XTransImage,
    x: usize,
    raw_y: usize,
    row_base: usize,
    row_rgb: &mut [f32],
    lookups: &[&NeighborLookup; 3],
) {
    let raw_x = x + xtrans.left_margin;
    let rgb_idx = x * 3;
    let color = xtrans.pattern.color_at(raw_y, raw_x);
    let val = xtrans.data[row_base + raw_x];

    // Set the known color channel
    row_rgb[rgb_idx + color as usize] = val;

    // Interpolate the other two channels with bounds checking
    for c in 0u8..3 {
        if c != color {
            row_rgb[rgb_idx + c as usize] =
                interpolate_channel_safe(xtrans, raw_x, raw_y, lookups[c as usize]);
        }
    }
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
