//! ARM aarch64 NEON SIMD implementation of X-Trans bilinear demosaicing.
//!
//! This implementation processes rows using SIMD for the neighbor summation.
//! The key optimization is vectorizing the sum of neighbor values.

use super::XTransImage;
use super::scalar::NeighborLookup;

/// Search radius for interpolation (5x5 neighborhood).
const SEARCH_RADIUS: usize = 2;

/// Process a row of X-Trans image data using NEON SIMD.
///
/// The SIMD optimization focuses on vectorizing the neighbor value summation
/// for interior pixels where we can safely access all neighbors without bounds checking.
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

    // Determine safe range for interior processing
    let interior_start = if xtrans.left_margin >= SEARCH_RADIUS {
        0
    } else {
        SEARCH_RADIUS - xtrans.left_margin
    };

    let interior_end = if xtrans.left_margin + xtrans.width + SEARCH_RADIUS <= xtrans.raw_width {
        xtrans.width
    } else {
        xtrans
            .raw_width
            .saturating_sub(xtrans.left_margin + SEARCH_RADIUS)
    };

    // Process interior pixels with SIMD-optimized interpolation if possible
    if is_interior_y && interior_end > interior_start {
        // Process left edge pixels with safe scalar
        for x in 0..interior_start {
            process_pixel_safe(xtrans, x, raw_y, row_base, row_rgb, lookups);
        }

        // Process interior pixels with SIMD-optimized interpolation
        for x in interior_start..interior_end {
            let raw_x = x + xtrans.left_margin;
            let rgb_idx = x * 3;
            let color = xtrans.pattern.color_at(raw_y, raw_x);
            let center_val = xtrans.data[row_base + raw_x];

            // Set the known color channel
            row_rgb[rgb_idx + color as usize] = center_val;

            // Interpolate the other two channels using SIMD
            for c in 0u8..3 {
                if c != color {
                    let offset_list = lookups[c as usize].get(raw_y, raw_x);
                    let offsets = offset_list.as_slice();
                    let inv_len = offset_list.inv_len();

                    // Use SIMD to sum neighbor values
                    // SAFETY: We're in an unsafe fn with neon target feature
                    let sum = unsafe { sum_neighbors_simd(xtrans, raw_x, raw_y, offsets) };
                    row_rgb[rgb_idx + c as usize] = sum * inv_len;
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

/// Sum neighbor values using NEON SIMD.
/// Processes 4 neighbors at a time.
#[target_feature(enable = "neon")]
#[inline]
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn sum_neighbors_simd(
    xtrans: &XTransImage,
    raw_x: usize,
    raw_y: usize,
    offsets: &[(i32, i32)],
) -> f32 {
    use std::arch::aarch64::*;

    let data_ptr = xtrans.data.as_ptr();
    let raw_width = xtrans.raw_width;

    // Process 4 neighbors at a time
    let chunks = offsets.len() / 4;
    let remainder = offsets.len() % 4;

    let mut sum_vec = vdupq_n_f32(0.0);

    // Process chunks of 4
    for chunk_idx in 0..chunks {
        let base = chunk_idx * 4;

        // Gather 4 neighbor values
        let (dy0, dx0) = offsets[base];
        let (dy1, dx1) = offsets[base + 1];
        let (dy2, dx2) = offsets[base + 2];
        let (dy3, dx3) = offsets[base + 3];

        let idx0 = ((raw_y as i32 + dy0) as usize) * raw_width + (raw_x as i32 + dx0) as usize;
        let idx1 = ((raw_y as i32 + dy1) as usize) * raw_width + (raw_x as i32 + dx1) as usize;
        let idx2 = ((raw_y as i32 + dy2) as usize) * raw_width + (raw_x as i32 + dx2) as usize;
        let idx3 = ((raw_y as i32 + dy3) as usize) * raw_width + (raw_x as i32 + dx3) as usize;

        // SAFETY: We've verified these indices are valid for interior pixels
        let v0 = *data_ptr.add(idx0);
        let v1 = *data_ptr.add(idx1);
        let v2 = *data_ptr.add(idx2);
        let v3 = *data_ptr.add(idx3);

        // Create vector from 4 values
        let vals_arr = [v0, v1, v2, v3];
        let vals = vld1q_f32(vals_arr.as_ptr());
        sum_vec = vaddq_f32(sum_vec, vals);
    }

    // Horizontal sum of the vector using NEON pairwise addition
    let sum_pair = vpaddq_f32(sum_vec, sum_vec); // [a+b, c+d, a+b, c+d]
    let sum_final = vpaddq_f32(sum_pair, sum_pair); // [a+b+c+d, ...]
    let mut total = vgetq_lane_f32(sum_final, 0);

    // Handle remainder with scalar
    for i in 0..remainder {
        let (dy, dx) = offsets[chunks * 4 + i];
        let idx = ((raw_y as i32 + dy) as usize) * raw_width + (raw_x as i32 + dx) as usize;
        // SAFETY: We've verified these indices are valid for interior pixels
        total += *data_ptr.add(idx);
    }

    total
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
