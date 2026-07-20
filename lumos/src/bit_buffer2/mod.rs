//! Bit-packed 2D buffer for boolean masks.
//!
//! Uses 1 bit per element instead of 1 byte, reducing memory by 8x.
//! Rows are padded to 128-bit boundaries for efficient word-based operations.
//! `(x, y)` and linear indexing like a dense `Vec<bool>` 2D grid, but bit-packed.

use std::ops::Index;

/// Number of bits per storage word.
const BITS_PER_WORD: usize = 64;

/// Row alignment in bits (128 bits = 16 bytes = 2 words).
const ROW_ALIGNMENT_BITS: usize = 128;

#[derive(Debug)]
struct BitLayout {
    stride: usize,
    len: usize,
    num_words: usize,
}

fn bit_layout(width: usize, height: usize) -> BitLayout {
    if width == 0 || height == 0 {
        return BitLayout {
            stride: 0,
            len: 0,
            num_words: 0,
        };
    }

    let stride = width
        .div_ceil(ROW_ALIGNMENT_BITS)
        .checked_mul(ROW_ALIGNMENT_BITS)
        .expect("BitBuffer2 row stride overflow");
    let len = width
        .checked_mul(height)
        .expect("BitBuffer2 dimensions overflow");
    let total_bits = stride
        .checked_mul(height)
        .expect("BitBuffer2 storage size overflow");
    debug_assert_eq!(total_bits % BITS_PER_WORD, 0);
    BitLayout {
        stride,
        len,
        num_words: total_bits / BITS_PER_WORD,
    }
}

/// A 2D buffer storing boolean values packed as bits.
///
/// Uses `u64` words internally, with 64 bits per word.
/// Rows are padded to 128 bits (2 words) for efficient word access.
/// This reduces memory usage by 8x compared to `Vec<bool>`.
#[derive(Debug, Clone)]
pub(crate) struct BitBuffer2 {
    /// Packed bit storage. Each u64 holds 64 boolean values.
    pub(crate) words: Vec<u64>,
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) len: usize,
    /// Number of bits per row including padding (aligned to 128 bits).
    pub(crate) stride: usize,
}

impl BitBuffer2 {
    /// Create a new bit buffer filled with the given value.
    #[inline]
    pub(crate) fn new_filled(width: usize, height: usize, value: bool) -> Self {
        let layout = bit_layout(width, height);
        let fill = if value { !0u64 } else { 0u64 };
        let words = vec![fill; layout.num_words];
        Self {
            words,
            width,
            height,
            len: layout.len,
            stride: layout.stride,
        }
    }

    /// Create a new bit buffer with all bits set to false.
    #[inline]
    pub(crate) fn new_default(width: usize, height: usize) -> Self {
        Self::new_filled(width, height, false)
    }

    /// Get the number of words per row.
    #[inline]
    pub(crate) fn words_per_row(&self) -> usize {
        self.stride / BITS_PER_WORD
    }

    /// Get a bit value at the given linear index (row-major, no padding).
    #[inline]
    pub(crate) fn get(&self, idx: usize) -> bool {
        debug_assert!(idx < self.len);
        let x = idx % self.width;
        let y = idx / self.width;
        self.get_xy(x, y)
    }

    /// Set a bit value at the given linear index (row-major, no padding).
    #[inline]
    pub(crate) fn set(&mut self, idx: usize, value: bool) {
        debug_assert!(idx < self.len);
        let x = idx % self.width;
        let y = idx / self.width;
        self.set_xy(x, y, value);
    }

    /// Get a bit value at the given (x, y) coordinates.
    #[inline]
    pub(crate) fn get_xy(&self, x: usize, y: usize) -> bool {
        debug_assert!(x < self.width && y < self.height);
        let bit_idx = y * self.stride + x;
        let word_idx = bit_idx / BITS_PER_WORD;
        let bit_in_word = bit_idx % BITS_PER_WORD;
        (self.words[word_idx] >> bit_in_word) & 1 != 0
    }

    /// Set a bit value at the given (x, y) coordinates.
    #[inline]
    pub(crate) fn set_xy(&mut self, x: usize, y: usize, value: bool) {
        debug_assert!(x < self.width && y < self.height);
        let bit_idx = y * self.stride + x;
        let word_idx = bit_idx / BITS_PER_WORD;
        let bit_in_word = bit_idx % BITS_PER_WORD;
        if value {
            self.words[word_idx] |= 1u64 << bit_in_word;
        } else {
            self.words[word_idx] &= !(1u64 << bit_in_word);
        }
    }

    /// Fill all bits with the given value.
    #[inline]
    pub(crate) fn fill(&mut self, value: bool) {
        let fill = if value { !0u64 } else { 0u64 };
        self.words.fill(fill);
    }

    /// Copy contents from another BitBuffer2.
    #[inline]
    pub(crate) fn copy_from(&mut self, other: &Self) {
        assert_eq!(self.width, other.width, "width mismatch");
        assert_eq!(self.height, other.height, "height mismatch");
        assert_eq!(self.stride, other.stride, "stride mismatch");
        self.words.copy_from_slice(&other.words);
    }

    /// Count the number of set bits (true values, excluding padding).
    #[inline]
    pub(crate) fn count_ones(&self) -> usize {
        if self.width == 0 || self.height == 0 {
            return 0;
        }

        let words_per_row = self.words_per_row();
        let bits_in_last_word = self.width % BITS_PER_WORD;
        // Number of fully-used words (all 64 bits valid)
        let full_words_per_row = self.width / BITS_PER_WORD;

        let mut count = 0usize;

        for y in 0..self.height {
            let row_start = y * words_per_row;

            // Count full words (all 64 bits valid)
            for w in 0..full_words_per_row {
                count += self.words[row_start + w].count_ones() as usize;
            }

            // Handle partial last word if width is not a multiple of 64.
            // The empty-buffer early-return guarantees width > 0 here, so a
            // nonzero `bits_in_last_word` already implies a valid last word.
            if bits_in_last_word != 0 {
                let last_word = self.words[row_start + full_words_per_row];
                // Mask off padding bits
                let mask = (1u64 << bits_in_last_word) - 1;
                count += (last_word & mask).count_ones() as usize;
            }
        }

        count
    }

    /// Iterate over all bit values (row-major order, skipping padding).
    #[inline]
    pub(crate) fn iter(&self) -> BitIter<'_> {
        BitIter {
            buffer: self,
            index: 0,
        }
    }
}

/// Index by linear index.
impl Index<usize> for BitBuffer2 {
    type Output = bool;

    #[inline]
    fn index(&self, idx: usize) -> &Self::Output {
        // We can't return a reference to a bit, so we use a static bool
        // This is a limitation of bit-packed storage
        if self.get(idx) { &true } else { &false }
    }
}

/// Index by (x, y) coordinates.
impl Index<(usize, usize)> for BitBuffer2 {
    type Output = bool;

    #[inline]
    fn index(&self, (x, y): (usize, usize)) -> &Self::Output {
        if self.get_xy(x, y) { &true } else { &false }
    }
}

// Note: IndexMut cannot be implemented for bit-packed storage because we cannot
// return a mutable reference to a single bit. Use `set(idx, value)` or
// `set_xy(x, y, value)` methods instead.

/// Convert BitBuffer2 to Vec<bool>.
impl From<BitBuffer2> for Vec<bool> {
    #[inline]
    fn from(buf: BitBuffer2) -> Self {
        buf.iter().collect()
    }
}

/// Convert &BitBuffer2 to Vec<bool>.
impl From<&BitBuffer2> for Vec<bool> {
    #[inline]
    fn from(buf: &BitBuffer2) -> Self {
        buf.iter().collect()
    }
}

/// Iterator over bit values (row-major order, skipping padding).
#[derive(Debug)]
pub(crate) struct BitIter<'a> {
    buffer: &'a BitBuffer2,
    index: usize,
}

impl Iterator for BitIter<'_> {
    type Item = bool;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        if self.index == self.buffer.len {
            return None;
        }

        let value = self.buffer.get(self.index);
        self.index += 1;
        Some(value)
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.buffer.len - self.index;
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for BitIter<'_> {}

#[cfg(test)]
mod test_support {
    use crate::bit_buffer2::{BitBuffer2, bit_layout};

    impl BitBuffer2 {
        pub(crate) fn from_slice(width: usize, height: usize, data: &[bool]) -> Self {
            let layout = bit_layout(width, height);
            assert_eq!(
                data.len(),
                layout.len,
                "data length {} does not match dimensions {}x{}={}",
                data.len(),
                width,
                height,
                layout.len
            );

            let mut buffer = Self::new_default(width, height);
            for (index, &value) in data.iter().enumerate() {
                if value {
                    buffer.set(index, true);
                }
            }
            buffer
        }
    }
}

#[cfg(test)]
mod tests;
