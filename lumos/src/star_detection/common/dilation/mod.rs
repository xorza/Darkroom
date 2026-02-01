//! Morphological dilation for binary masks.
//!
//! This module provides efficient dilation operations on bit buffers,
//! used for connecting nearby pixels in star detection and background masking.

#[cfg(test)]
mod bench;
#[cfg(test)]
mod tests;

use crate::common::BitBuffer2;
use rayon::iter::{IntoParallelIterator, ParallelIterator};

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

    // Horizontal dilation pass
    (0..height).into_par_iter().for_each(|y| {
        let row_start = y * words_per_row;
        let input_words = mask.words();
        // SAFETY: Each thread writes to a disjoint set of rows
        let output_words = unsafe {
            std::slice::from_raw_parts_mut(output.words().as_ptr() as *mut u64, output.num_words())
        };

        for word_idx in 0..words_per_row {
            let base_x = word_idx * 64;
            let row = &input_words[row_start..row_start + words_per_row];

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

    // Vertical dilation pass
    (0..words_per_row).into_par_iter().for_each(|word_idx| {
        // SAFETY: Each thread accesses a disjoint word index across all rows
        let output_words = unsafe {
            std::slice::from_raw_parts_mut(output.words().as_ptr() as *mut u64, output.num_words())
        };

        let original: Vec<u64> = (0..height)
            .map(|y| output_words[y * words_per_row + word_idx])
            .collect();

        for y in 0..height {
            let y_min = y.saturating_sub(radius);
            let y_max = (y + radius).min(height - 1);

            output_words[y * words_per_row + word_idx] = original[y_min..=y_max]
                .iter()
                .fold(0u64, |acc, &word| acc | word);
        }
    });
}

/// Fast horizontal dilation using word-level bit operations (radius <= 63).
#[inline]
fn dilate_word_fast(row: &[u64], word_idx: usize, radius: usize) -> u64 {
    let current = row[word_idx];
    let mut result = current;

    // Dilate within current word
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

/// Check if any bit is set in the given x range.
#[inline]
fn has_set_bit_in_range(row: &[u64], x_min: usize, x_max: usize) -> bool {
    let word_min = x_min / 64;
    let word_max = (x_max / 64).min(row.len() - 1);

    for (word_idx, &word) in row.iter().enumerate().take(word_max + 1).skip(word_min) {
        if word == 0 {
            continue;
        }

        let word_start = word_idx * 64;
        for bit in 0..64 {
            let x = word_start + bit;
            if x < x_min {
                continue;
            }
            if x > x_max {
                break;
            }
            if (word >> bit) & 1 != 0 {
                return true;
            }
        }
    }

    false
}
