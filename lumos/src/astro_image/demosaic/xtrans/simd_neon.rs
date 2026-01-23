//! ARM aarch64 NEON SIMD implementation of X-Trans bilinear demosaicing.
//!
//! This implementation uses pre-computed linear offsets and unrolled loops
//! to maximize performance for X-Trans sensor demosaicing.

use super::XTransImage;
use super::scalar::{LinearNeighborLookup, NeighborLookup};

/// Search radius for interpolation (5x5 neighborhood).
const SEARCH_RADIUS: usize = 2;

/// Process a row of X-Trans image data using pre-computed linear offsets.
///
/// # Safety
/// Caller must ensure NEON is available on the target CPU.
#[target_feature(enable = "neon")]
#[allow(unsafe_op_in_unsafe_fn)]
pub(crate) unsafe fn process_row_simd_linear(
    xtrans: &XTransImage,
    y: usize,
    row_rgb: &mut [f32],
    lookups: &[&NeighborLookup; 3],
    linear_lookups: &[&LinearNeighborLookup; 3],
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

        // Process interior pixels with optimized linear access
        let data_ptr = xtrans.data.as_ptr();

        for x in interior_start..interior_end {
            let raw_x = x + xtrans.left_margin;
            let rgb_idx = x * 3;
            let color = xtrans.pattern.color_at(raw_y, raw_x);
            let base_idx = row_base + raw_x;
            let center_val = *data_ptr.add(base_idx);

            // Set the known color channel
            *row_rgb.get_unchecked_mut(rgb_idx + color as usize) = center_val;

            // Interpolate the other two channels using pre-computed linear offsets
            for c in 0u8..3 {
                if c != color {
                    let offset_list = linear_lookups[c as usize].get(raw_y, raw_x);
                    let offsets = offset_list.as_slice();
                    let inv_len = offset_list.inv_len();

                    let sum = sum_neighbors_linear(data_ptr, base_idx, offsets);
                    *row_rgb.get_unchecked_mut(rgb_idx + c as usize) = sum * inv_len;
                }
            }
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

/// Sum neighbor values using pre-computed linear offsets.
/// Uses 4 independent accumulators for instruction-level parallelism.
#[inline(always)]
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn sum_neighbors_linear(data_ptr: *const f32, base_idx: usize, offsets: &[isize]) -> f32 {
    let base_ptr = data_ptr.add(base_idx);

    // Use 4 accumulators for better ILP
    let mut sum0 = 0.0f32;
    let mut sum1 = 0.0f32;
    let mut sum2 = 0.0f32;
    let mut sum3 = 0.0f32;

    let len = offsets.len();
    let chunks = len / 4;
    let remainder = len % 4;

    // Process in chunks of 4
    for chunk in 0..chunks {
        let base = chunk * 4;

        let off0 = *offsets.get_unchecked(base);
        let off1 = *offsets.get_unchecked(base + 1);
        let off2 = *offsets.get_unchecked(base + 2);
        let off3 = *offsets.get_unchecked(base + 3);

        sum0 += *base_ptr.offset(off0);
        sum1 += *base_ptr.offset(off1);
        sum2 += *base_ptr.offset(off2);
        sum3 += *base_ptr.offset(off3);
    }

    // Handle remainder
    let rem_base = chunks * 4;
    for i in 0..remainder {
        let off = *offsets.get_unchecked(rem_base + i);
        sum0 += *base_ptr.offset(off);
    }

    sum0 + sum1 + sum2 + sum3
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
