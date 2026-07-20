//! Morphological dilation for binary masks.
//!
//! This module provides efficient dilation operations on bit buffers,
//! used for connecting nearby pixels in star detection and background masking.

#[cfg(test)]
mod bench;
#[cfg(test)]
mod tests;

use crate::bit_buffer2::BitBuffer2;
use crate::concurrency::UnsafeSendPtr;
use rayon::iter::{IndexedParallelIterator, IntoParallelIterator, ParallelIterator};
use rayon::slice::ParallelSliceMut;

/// Dilate a binary mask by the given radius (morphological dilation).
///
/// This connects nearby pixels that might be separated due to variable threshold.
/// Used in star detection to merge fragmented detections and in background
/// estimation to mask object wings.
///
/// Uses separable dilation (horizontal then vertical passes) for O(r) complexity
/// per pixel instead of O(r²). Operates on packed 64-bit words for efficiency.
pub(crate) fn dilate_mask(mask: &BitBuffer2, radius: usize, output: &mut BitBuffer2) {
    assert_eq!(mask.width, output.width, "width mismatch");
    assert_eq!(mask.height, output.height, "height mismatch");

    if radius == 0 {
        output.copy_from(mask);
        return;
    }
    // The fast word kernel smears within a 64-bit word, so a single pass covers radius ≤ 63.
    // Production never exceeds this (config caps `bg_mask_dilation` at 50).
    assert!(
        radius <= 63,
        "dilate_mask radius must be <= 63, got {radius}"
    );

    let width = mask.width;
    let words_per_row = mask.words_per_row();
    let input_words = &mask.words;

    // Horizontal dilation pass — rows are independent, so hand each task its own row slice.
    output
        .words
        .par_chunks_mut(words_per_row)
        .enumerate()
        .for_each(|(y, out_row)| {
            let row_start = y * words_per_row;
            let row = &input_words[row_start..row_start + words_per_row];

            for (word_idx, out) in out_row.iter_mut().enumerate() {
                let base_x = word_idx * 64;
                let mut result = dilate_word_fast(row, word_idx, radius);

                // Mask off bits beyond width for the last (partial) word.
                if base_x < width && base_x + 64 > width {
                    result &= (1u64 << (width - base_x)) - 1;
                }

                *out = result;
            }
        });

    // Vertical dilation pass with sliding window. Columns are strided across the row-major buffer,
    // so each task writes disjoint word columns through a shared raw pointer.
    let height = mask.height;
    let num_words = output.words.len();
    let out_ptr = UnsafeSendPtr::new(output.words.as_mut_ptr());
    let chunk_size = 64.max(words_per_row / rayon::current_num_threads().max(1));

    (0..words_per_row)
        .into_par_iter()
        .step_by(chunk_size)
        .for_each(|chunk_start| {
            let chunk_end = (chunk_start + chunk_size).min(words_per_row);

            // SAFETY: Each thread accesses disjoint word indices (non-overlapping chunk ranges).
            let p = out_ptr;
            let output_words = unsafe { std::slice::from_raw_parts_mut(p.get(), num_words) };

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
