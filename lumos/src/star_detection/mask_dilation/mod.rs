//! Morphological dilation for binary masks.
//!
//! This module provides efficient dilation operations on bit buffers,
//! used for connecting nearby pixels in star detection and background masking.

#[cfg(test)]
mod bench;
#[cfg(test)]
mod tests;

use crate::common::BitBuffer2;
use rayon::iter::{IndexedParallelIterator, IntoParallelIterator, ParallelIterator};

/// Wrapper to send raw pointers across thread boundaries.
/// SAFETY: Caller must ensure disjoint access from each thread.
#[derive(Clone, Copy)]
struct SendPtr(*mut u64);
unsafe impl Send for SendPtr {}
unsafe impl Sync for SendPtr {}

/// Dilate a binary mask by the given radius (morphological dilation).
///
/// This connects nearby pixels that might be separated due to variable threshold.
/// Used in star detection to merge fragmented detections and in background
/// estimation to mask object wings.
///
/// Uses separable dilation (horizontal then vertical passes) for O(r) complexity
/// per pixel instead of O(r²). Operates on packed 64-bit words for efficiency.
pub fn dilate_mask(mask: &BitBuffer2, radius: usize, output: &mut BitBuffer2) {
    assert_eq!(mask.width(), output.width(), "width mismatch");
    assert_eq!(mask.height(), output.height(), "height mismatch");

    if radius == 0 {
        output.copy_from(mask);
        return;
    }

    let width = mask.width();
    let height = mask.height();
    let words_per_row = mask.words_per_row();

    // Get mutable pointer from a proper &mut borrow before entering parallel sections
    let num_words = output.num_words();
    let out_ptr = SendPtr(output.words_mut().as_mut_ptr());

    // Horizontal dilation pass
    (0..height).into_par_iter().for_each(|y| {
        let row_start = y * words_per_row;
        let input_words = mask.words();
        // SAFETY: Each thread writes to a disjoint set of rows (non-overlapping row_start ranges).
        // Rebind to capture the SendPtr (which is Sync), not the raw pointer field.
        let p = out_ptr;
        let output_words = unsafe { std::slice::from_raw_parts_mut(p.0, num_words) };

        let row = &input_words[row_start..row_start + words_per_row];

        for word_idx in 0..words_per_row {
            let base_x = word_idx * 64;

            let mut result = if radius <= 63 {
                dilate_word_fast(row, word_idx, radius)
            } else {
                dilate_word_slow(row, word_idx, width, radius)
            };

            // Mask off bits beyond width for the last word
            if base_x < width && base_x + 64 > width {
                result &= (1u64 << (width - base_x)) - 1;
            } else if base_x >= width {
                result = 0;
            }

            output_words[row_start + word_idx] = result;
        }
    });

    // Vertical dilation pass with sliding window
    let chunk_size = 64.max(words_per_row / rayon::current_num_threads().max(1));

    (0..words_per_row)
        .into_par_iter()
        .step_by(chunk_size)
        .for_each(|chunk_start| {
            let chunk_end = (chunk_start + chunk_size).min(words_per_row);

            // SAFETY: Each thread accesses disjoint word indices (non-overlapping chunk ranges).
            let p = out_ptr;
            let output_words = unsafe { std::slice::from_raw_parts_mut(p.0, num_words) };

            let mut column_data = vec![0u64; height];
            let mut dilated = vec![0u64; height];

            for word_idx in chunk_start..chunk_end {
                // Read column
                for (y, col) in column_data.iter_mut().enumerate() {
                    *col = output_words[y * words_per_row + word_idx];
                }

                // Sliding window vertical dilation
                dilate_column_sliding(&column_data, &mut dilated, radius);

                // Write back
                for (y, &val) in dilated.iter().enumerate() {
                    output_words[y * words_per_row + word_idx] = val;
                }
            }
        });
}

/// Vertical dilation using sliding window - O(height) for sparse data.
///
/// For each position y, we need OR of column[y-radius..=y+radius].
/// Uses incremental OR with recomputation when the leaving element contributed bits.
/// This is O(height) for sparse masks (typical in star detection) since recomputation
/// is rare when most rows are zero.
#[inline]
fn dilate_column_sliding(column: &[u64], output: &mut [u64], radius: usize) {
    let height = column.len();
    if height == 0 {
        return;
    }

    // Initialize window OR for row 0
    let mut window_or = 0u64;
    let initial_end = radius.min(height - 1);
    for &val in &column[0..=initial_end] {
        window_or |= val;
    }
    output[0] = window_or;

    // Slide the window down
    for y in 1..height {
        // Remove the element leaving the window (if any)
        if y > radius {
            let leaving = column[y - radius - 1];
            if leaving != 0 && window_or & leaving != 0 {
                // Need to recompute - the leaving element contributed bits
                let y_min = y.saturating_sub(radius);
                let y_max = (y + radius).min(height - 1);
                window_or = column[y_min..=y_max].iter().fold(0u64, |acc, &v| acc | v);
            }
        }

        // Add the element entering the window (if any)
        let entering_y = y + radius;
        if entering_y < height {
            window_or |= column[entering_y];
        }

        output[y] = window_or;
    }
}

/// Fast horizontal dilation using word-level bit operations (radius <= 63).
#[inline]
fn dilate_word_fast(row: &[u64], word_idx: usize, radius: usize) -> u64 {
    let current = row[word_idx];
    let mut result = current;

    // Dilate within current word using bit smearing
    for shift in 1..=radius {
        result |= current << shift;
        result |= current >> shift;
    }

    // Left word contributes to our low bits
    if word_idx > 0 {
        let prev = row[word_idx - 1];
        if prev != 0 {
            for shift in 1..=radius {
                result |= prev >> (64 - shift);
            }
        }
    }

    // Right word contributes to our high bits
    if word_idx + 1 < row.len() {
        let next = row[word_idx + 1];
        if next != 0 {
            for shift in 1..=radius {
                result |= next << (64 - shift);
            }
        }
    }

    result
}

/// Slow horizontal dilation using per-bit checking (radius > 63).
#[inline]
fn dilate_word_slow(row: &[u64], word_idx: usize, width: usize, radius: usize) -> u64 {
    let base_x = word_idx * 64;
    let mut result = 0u64;

    for bit in 0..64usize {
        let x = base_x + bit;
        if x >= width {
            break;
        }

        let x_min = x.saturating_sub(radius);
        let x_max = (x + radius).min(width - 1);

        if has_set_bit_in_range(row, x_min, x_max) {
            result |= 1u64 << bit;
        }
    }

    result
}

/// Check if any bit is set in the given x range using word masks.
#[inline]
fn has_set_bit_in_range(row: &[u64], x_min: usize, x_max: usize) -> bool {
    let word_min = x_min / 64;
    let word_max = (x_max / 64).min(row.len() - 1);

    for (i, &word) in row[word_min..=word_max].iter().enumerate() {
        if word == 0 {
            continue;
        }

        let word_idx = word_min + i;
        let word_start = word_idx * 64;

        // Calculate bit range within this word
        let bit_start = x_min.saturating_sub(word_start);
        let bit_end = (x_max - word_start).min(63);

        // Create mask for the relevant bits
        let mask = if bit_end >= 63 {
            !0u64 << bit_start
        } else {
            ((1u64 << (bit_end + 1)) - 1) & (!0u64 << bit_start)
        };

        if word & mask != 0 {
            return true;
        }
    }

    false
}
