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
///
/// # Arguments
/// * `mask` - Input binary mask (BitBuffer2)
/// * `radius` - Dilation radius in pixels
/// * `output` - Output buffer for dilated mask (will be cleared and filled)
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
    // For each output bit at position x, we set it if any input bit in [x-radius, x+radius] is set
    (0..height).into_par_iter().for_each(|y| {
        let row_start = y * words_per_row;

        // SAFETY: Each thread writes to a disjoint set of rows
        let input_words = mask.words();
        let output_words = unsafe {
            std::slice::from_raw_parts_mut(output.words().as_ptr() as *mut u64, output.num_words())
        };

        for word_idx in 0..words_per_row {
            let base_x = word_idx * 64;
            let mut result = 0u64;

            // For small radii (<= 63), use fast word-level bit operations
            // For large radii, use per-bit checking
            if radius <= 63 {
                // Start with current word dilated within itself
                let current = input_words[row_start + word_idx];
                result |= current;
                for shift in 1..=radius {
                    result |= current << shift;
                    result |= current >> shift;
                }

                // Handle contributions from adjacent words for the boundary bits
                // Left word contributes to our low bits
                if word_idx > 0 {
                    let prev = input_words[row_start + word_idx - 1];
                    if prev != 0 {
                        for shift in 1..=radius {
                            result |= prev >> (64 - shift);
                        }
                    }
                }

                // Right word contributes to our high bits
                if word_idx + 1 < words_per_row {
                    let next = input_words[row_start + word_idx + 1];
                    if next != 0 {
                        for shift in 1..=radius {
                            result |= next << (64 - shift);
                        }
                    }
                }
            } else {
                // Large radius: use per-bit checking (slow but correct)
                for bit in 0..64usize {
                    let x = base_x + bit;
                    if x >= width {
                        break;
                    }

                    let x_min = x.saturating_sub(radius);
                    let x_max = (x + radius).min(width - 1);
                    let word_min = x_min / 64;
                    let word_max = (x_max / 64).min(words_per_row - 1);

                    let mut found = false;
                    for src_word_idx in word_min..=word_max {
                        let src_word = input_words[row_start + src_word_idx];
                        if src_word == 0 {
                            continue;
                        }

                        let src_base = src_word_idx * 64;
                        for src_bit in 0..64 {
                            let sx = src_base + src_bit;
                            if sx >= width {
                                break;
                            }
                            if sx < x_min || sx > x_max {
                                continue;
                            }
                            if (src_word >> src_bit) & 1 != 0 {
                                found = true;
                                break;
                            }
                        }
                        if found {
                            break;
                        }
                    }

                    if found {
                        result |= 1u64 << bit;
                    }
                }
            }

            // Mask off bits beyond width for the last word
            if base_x < width && base_x + 64 > width {
                let valid_bits = width - base_x;
                if valid_bits < 64 {
                    result &= (1u64 << valid_bits) - 1;
                }
            } else if base_x >= width {
                result = 0;
            }

            output_words[row_start + word_idx] = result;
        }
    });

    // Vertical dilation pass: process words instead of individual columns
    // This is more cache-friendly as we access consecutive memory
    (0..words_per_row).into_par_iter().for_each(|word_idx| {
        // SAFETY: Each thread accesses a disjoint word index across all rows
        let output_words = unsafe {
            std::slice::from_raw_parts_mut(output.words().as_ptr() as *mut u64, output.num_words())
        };

        // Store original values since we're reading and writing the same buffer
        let original: Vec<u64> = (0..height)
            .map(|y| output_words[y * words_per_row + word_idx])
            .collect();

        // For each row, OR together all words in [y-radius, y+radius]
        for y in 0..height {
            let y_min = y.saturating_sub(radius);
            let y_max = (y + radius).min(height - 1);

            let dilated = original[y_min..=y_max]
                .iter()
                .fold(0u64, |acc, &word| acc | word);

            output_words[y * words_per_row + word_idx] = dilated;
        }
    });
}
