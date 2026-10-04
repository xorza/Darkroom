//! The disk dilation [`BitBuffer2::dilate`] runs.

use rayon::iter::{IndexedParallelIterator, ParallelIterator};
use rayon::slice::ParallelSliceMut;

use crate::bit_buffer2::BitBuffer2;

/// Dilate `mask` in place by a disk of `radius`: a pixel is set when a set pixel lies within
/// Euclidean distance `radius` of it. `scratch`, of any contents and the mask's size, holds the
/// source while the mask is rewritten.
///
/// A disk rather than a square, as photutils dilates its source masks: a square reaches √2 times
/// further along the diagonals and masks sky that no source touches.
///
/// Each output row ORs the rows within `radius` of it, each smeared horizontally by the disk's
/// half-width at that offset, `⌊√(radius² − dy²)⌋`, on packed 64-bit words. Rows run in parallel
/// over contiguous memory.
pub(super) fn dilate_mask(mask: &mut BitBuffer2, radius: usize, scratch: &mut BitBuffer2) {
    assert_eq!(mask.size, scratch.size, "size mismatch");
    if radius == 0 {
        return;
    }
    // The word kernel smears within a 64-bit word, so a single pass covers radius ≤ 63.
    assert!(radius <= 63, "dilation radius must be <= 63, got {radius}");

    let width = mask.size.width;
    let height = mask.size.height;
    let words_per_row = mask.words_per_row();
    if words_per_row == 0 {
        return;
    }
    // The words that hold pixels; a row's stride may pad it with more, which stay clear. The smear
    // must not set the bits past the width in the last of them either.
    let used_words = width.div_ceil(64);
    let last_word_bits = width - (used_words - 1) * 64;
    let last_word_mask = if last_word_bits == 64 {
        u64::MAX
    } else {
        (1u64 << last_word_bits) - 1
    };

    scratch.words.copy_from_slice(&mask.words);
    let source = &scratch.words;
    mask.words
        .par_chunks_mut(words_per_row)
        .enumerate()
        .for_each(|(y, out_row)| {
            out_row.fill(0);
            for source_y in y.saturating_sub(radius)..=(y + radius).min(height - 1) {
                let offset = source_y.abs_diff(y);
                let reach = (radius * radius - offset * offset).isqrt();
                let row = &source[source_y * words_per_row..][..used_words];
                for (word_idx, out) in out_row[..used_words].iter_mut().enumerate() {
                    *out |= dilate_word_fast(row, word_idx, reach);
                }
            }
            out_row[used_words - 1] &= last_word_mask;
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
