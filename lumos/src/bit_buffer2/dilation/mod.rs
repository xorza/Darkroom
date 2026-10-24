//! The disk dilation [`BitBuffer2::dilate`] runs.

use common::par;
use rayon::iter::{IndexedParallelIterator, ParallelIterator};

use crate::bit_buffer2::BitBuffer2;

/// Dilate `mask` in place by a disk of `radius`: a pixel is set when a set pixel lies within
/// Euclidean distance `radius` of it. `scratch`, of any contents and the mask's size, holds the
/// source while the mask is rewritten.
///
/// A disk rather than a square, as photutils dilates its source masks: a square reaches √2 times
/// further along the diagonals and masks sky that no source touches.
///
/// Each output row ORs the rows within `radius` of it, each smeared horizontally by the disk's
/// half-width at that offset, `⌊√(radius² − dy²)⌋`, on packed 64-bit words. A source word that it
/// and its neighbours leave clear adds nothing and is skipped, so a sparse mask costs a load per
/// word and offset. Rows run in parallel over contiguous memory.
pub(super) fn dilate_mask(mask: &mut BitBuffer2, radius: usize, scratch: &mut BitBuffer2) {
    debug_assert_eq!(mask.size, scratch.size, "size mismatch");
    if radius == 0 {
        return;
    }
    // A smear reaches one word either way, so a single pass covers radius ≤ 63.
    debug_assert!(radius <= 63, "dilation radius must be <= 63, got {radius}");

    let width = mask.size.width;
    let height = mask.size.height;
    let words_per_row = mask.words_per_row();
    if words_per_row == 0 {
        return;
    }
    // The words that hold pixels; a row's stride may pad it with more, which stay clear. The smear
    // must not set the bits past the width in the last of them either.
    let used_words = width.div_ceil(64);
    let last_word_mask = mask.last_word_mask();
    let mut reaches = [0; 64];
    for (offset, reach) in reaches.iter_mut().enumerate().take(radius + 1) {
        *reach = (radius * radius - offset * offset).isqrt();
    }

    scratch.words.copy_from_slice(&mask.words);
    let source = &scratch.words;
    par::chunks_mut(&mut mask.words, words_per_row)
        .enumerate()
        .for_each(|(y, out_row)| {
            out_row.fill(0);
            for source_y in y.saturating_sub(radius)..=(y + radius).min(height - 1) {
                let reach = reaches[source_y.abs_diff(y)];
                let row = &source[source_y * words_per_row..][..used_words];
                for (word_idx, out) in out_row[..used_words].iter_mut().enumerate() {
                    let prev = if word_idx > 0 { row[word_idx - 1] } else { 0 };
                    let next = row.get(word_idx + 1).copied().unwrap_or(0);
                    let cur = row[word_idx];
                    if prev | cur | next != 0 {
                        *out |= smear(prev, cur, next, reach);
                    }
                }
            }
            out_row[used_words - 1] &= last_word_mask;
        });
}

/// `cur` smeared `reach` bits either way, the bits `prev` and `next` reach into it
/// included (`reach` ≤ 63).
#[inline]
fn smear(prev: u64, cur: u64, next: u64, reach: usize) -> u64 {
    let up = smear_up((u128::from(cur) << 64) | u128::from(prev), reach) >> 64;
    let down = smear_down((u128::from(next) << 64) | u128::from(cur), reach);
    (up | down) as u64
}

/// `bits` ORed with itself shifted up by every distance to `reach`: doubled while the shifts it
/// covers, `0..covered`, fit, then shifted once more by what remains, which overlaps them.
#[inline]
const fn smear_up(mut bits: u128, reach: usize) -> u128 {
    let mut covered = 1;
    while 2 * covered <= reach + 1 {
        bits |= bits << covered;
        covered *= 2;
    }
    if covered <= reach {
        bits |= bits << (reach + 1 - covered);
    }
    bits
}

/// [`smear_up`] downward.
#[inline]
const fn smear_down(mut bits: u128, reach: usize) -> u128 {
    let mut covered = 1;
    while 2 * covered <= reach + 1 {
        bits |= bits >> covered;
        covered *= 2;
    }
    if covered <= reach {
        bits |= bits >> (reach + 1 - covered);
    }
    bits
}

#[cfg(test)]
mod tests;
