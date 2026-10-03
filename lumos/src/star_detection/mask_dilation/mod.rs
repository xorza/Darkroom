//! Morphological dilation for binary masks.
//!
//! This module provides efficient dilation operations on bit buffers,
//! used for connecting nearby pixels in star detection and background masking.

use rayon::iter::{IndexedParallelIterator, ParallelIterator};
use rayon::slice::ParallelSliceMut;

use crate::bit_buffer2::BitBuffer2;

/// Dilate `mask` in place by `radius` with a square structuring element (morphological dilation),
/// using `scratch` — any contents, the mask's size — for the intermediate pass.
///
/// This connects nearby pixels that might be separated due to variable threshold.
/// Used in star detection to merge fragmented detections and in background
/// estimation to mask object wings.
///
/// Separable: a horizontal pass on packed 64-bit words into `scratch`, then a vertical pass that
/// ORs each output row's `2·radius + 1` neighbouring rows back into `mask`. Both passes run row
/// by row in parallel over contiguous memory, at O(radius) word operations per word.
pub(crate) fn dilate_mask(mask: &mut BitBuffer2, radius: usize, scratch: &mut BitBuffer2) {
    assert_eq!(mask.size, scratch.size, "size mismatch");
    if radius == 0 {
        return;
    }
    // The word kernel smears within a 64-bit word, so a single pass covers radius ≤ 63.
    assert!(
        radius <= 63,
        "dilate_mask radius must be <= 63, got {radius}"
    );

    let width = mask.size.width;
    let height = mask.size.height;
    let words_per_row = mask.words_per_row();
    if words_per_row == 0 {
        return;
    }

    let input = &mask.words;
    scratch
        .words
        .par_chunks_mut(words_per_row)
        .enumerate()
        .for_each(|(y, out_row)| {
            let row = &input[y * words_per_row..(y + 1) * words_per_row];
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

    let horizontal = &scratch.words;
    mask.words
        .par_chunks_mut(words_per_row)
        .enumerate()
        .for_each(|(y, out_row)| {
            let first = y.saturating_sub(radius);
            let last = (y + radius).min(height - 1);
            out_row
                .copy_from_slice(&horizontal[first * words_per_row..(first + 1) * words_per_row]);
            for source in first + 1..=last {
                let row = &horizontal[source * words_per_row..(source + 1) * words_per_row];
                for (out, &word) in out_row.iter_mut().zip(row) {
                    *out |= word;
                }
            }
        });
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

#[cfg(all(test, feature = "bench"))]
mod bench;
#[cfg(test)]
mod tests;
