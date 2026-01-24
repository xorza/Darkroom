//! ARM aarch64 NEON SIMD implementation of X-Trans bilinear demosaicing.
//!
//! This implementation uses:
//! - Pre-computed linear offsets for fast neighbor access
//! - NEON vector operations for parallel accumulation
//! - 4-way unrolling with vector intrinsics for maximum throughput

use super::XTransImage;
use super::scalar::{LinearNeighborLookup, NeighborLookup};

/// Search radius for interpolation (5x5 neighborhood).
const SEARCH_RADIUS: usize = 2;

/// Process a row of X-Trans image data using NEON SIMD acceleration.
///
/// # Safety
/// Caller must ensure NEON is available on the target CPU.
#[target_feature(enable = "neon")]
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

        // Process interior pixels with optimized NEON access
        // SAFETY: We're in an unsafe function and have verified interior bounds
        unsafe {
            process_interior_pixels_neon(
                xtrans,
                raw_y,
                row_base,
                interior_start,
                interior_end,
                row_rgb,
                linear_lookups,
            );
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

/// Process interior pixels using NEON-optimized accumulation.
///
/// # Safety
/// Caller must ensure:
/// - NEON is available
/// - All pixel accesses within [interior_start, interior_end) are safe
/// - raw_y is within safe vertical bounds
#[target_feature(enable = "neon")]
#[inline]
unsafe fn process_interior_pixels_neon(
    xtrans: &XTransImage,
    raw_y: usize,
    row_base: usize,
    interior_start: usize,
    interior_end: usize,
    row_rgb: &mut [f32],
    linear_lookups: &[&LinearNeighborLookup; 3],
) {
    let data_ptr = xtrans.data.as_ptr();

    for x in interior_start..interior_end {
        let raw_x = x + xtrans.left_margin;
        let rgb_idx = x * 3;
        let color = xtrans.pattern.color_at(raw_y, raw_x);
        let base_idx = row_base + raw_x;

        // SAFETY: base_idx is within bounds (verified by interior check)
        let center_val = unsafe { *data_ptr.add(base_idx) };

        // Set the known color channel
        // SAFETY: rgb_idx + color < row_rgb.len() (verified by caller)
        unsafe {
            *row_rgb.get_unchecked_mut(rgb_idx + color as usize) = center_val;
        }

        // Interpolate the other two channels using pre-computed linear offsets
        for c in 0u8..3 {
            if c != color {
                let offset_list = linear_lookups[c as usize].get(raw_y, raw_x);
                let offsets = offset_list.as_slice();
                let inv_len = offset_list.inv_len();

                // SAFETY: All offsets are pre-validated to be within bounds
                let sum = unsafe { sum_neighbors_neon(data_ptr, base_idx, offsets) };

                // SAFETY: rgb_idx + c < row_rgb.len()
                unsafe {
                    *row_rgb.get_unchecked_mut(rgb_idx + c as usize) = sum * inv_len;
                }
            }
        }
    }
}

/// Sum neighbor values using NEON vector operations.
///
/// Uses NEON `vaddq_f32` for 4-wide parallel accumulation when possible,
/// with scalar fallback for remainders.
///
/// # Safety
/// Caller must ensure all `base_idx + offset` values are valid indices into the data.
#[inline]
#[target_feature(enable = "neon")]
unsafe fn sum_neighbors_neon(data_ptr: *const f32, base_idx: usize, offsets: &[isize]) -> f32 {
    use std::arch::aarch64::*;

    // SAFETY: All operations in this function are safe because:
    // - The caller guarantees base_idx + offsets are valid
    // - NEON is guaranteed by target_feature attribute
    unsafe {
        let base_ptr = data_ptr.add(base_idx);
        let len = offsets.len();
        if len >= 8 {
            // Use NEON for larger neighbor counts (typical for X-Trans)
            // Process 4 offsets at a time with two vector accumulators
            let mut acc0 = vdupq_n_f32(0.0);
            let mut acc1 = vdupq_n_f32(0.0);

            let chunks = len / 8;
            let remainder_start = chunks * 8;

            for chunk in 0..chunks {
                let base = chunk * 8;

                // Load 8 values using two sets of 4 scalar loads
                // (offsets are irregular so we can't use vector loads directly)
                let off0 = *offsets.get_unchecked(base);
                let off1 = *offsets.get_unchecked(base + 1);
                let off2 = *offsets.get_unchecked(base + 2);
                let off3 = *offsets.get_unchecked(base + 3);
                let off4 = *offsets.get_unchecked(base + 4);
                let off5 = *offsets.get_unchecked(base + 5);
                let off6 = *offsets.get_unchecked(base + 6);
                let off7 = *offsets.get_unchecked(base + 7);

                // Load values and pack into vectors
                let v0 = *base_ptr.offset(off0);
                let v1 = *base_ptr.offset(off1);
                let v2 = *base_ptr.offset(off2);
                let v3 = *base_ptr.offset(off3);
                let v4 = *base_ptr.offset(off4);
                let v5 = *base_ptr.offset(off5);
                let v6 = *base_ptr.offset(off6);
                let v7 = *base_ptr.offset(off7);

                // Create vectors from loaded values
                let vals0 = vld1q_f32([v0, v1, v2, v3].as_ptr());
                let vals1 = vld1q_f32([v4, v5, v6, v7].as_ptr());

                // Accumulate
                acc0 = vaddq_f32(acc0, vals0);
                acc1 = vaddq_f32(acc1, vals1);
            }

            // Combine the two accumulators
            acc0 = vaddq_f32(acc0, acc1);

            // Horizontal sum of acc0
            let sum_vec = vpaddq_f32(acc0, acc0);
            let sum_pair = vgetq_lane_f32::<0>(sum_vec) + vgetq_lane_f32::<1>(sum_vec);

            // Handle remainder with scalar
            let mut scalar_sum = sum_pair;
            for i in remainder_start..len {
                let off = *offsets.get_unchecked(i);
                scalar_sum += *base_ptr.offset(off);
            }

            scalar_sum
        } else if len >= 4 {
            // Use NEON for 4-7 elements
            let mut acc = vdupq_n_f32(0.0);

            // Process first 4 elements
            let off0 = *offsets.get_unchecked(0);
            let off1 = *offsets.get_unchecked(1);
            let off2 = *offsets.get_unchecked(2);
            let off3 = *offsets.get_unchecked(3);

            let v0 = *base_ptr.offset(off0);
            let v1 = *base_ptr.offset(off1);
            let v2 = *base_ptr.offset(off2);
            let v3 = *base_ptr.offset(off3);

            let vals = vld1q_f32([v0, v1, v2, v3].as_ptr());
            acc = vaddq_f32(acc, vals);

            // Horizontal sum
            let sum_vec = vpaddq_f32(acc, acc);
            let mut sum = vgetq_lane_f32::<0>(sum_vec) + vgetq_lane_f32::<1>(sum_vec);

            // Handle remainder with scalar
            for i in 4..len {
                let off = *offsets.get_unchecked(i);
                sum += *base_ptr.offset(off);
            }

            sum
        } else {
            // Fall back to scalar for very small counts
            let mut sum = 0.0f32;
            for i in 0..len {
                let off = *offsets.get_unchecked(i);
                sum += *base_ptr.offset(off);
            }
            sum
        }
    }
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
