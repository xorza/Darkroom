//! Bit-packed 2D buffer for boolean masks.
//!
//! Uses 1 bit per element instead of 1 byte, reducing memory by 8x.
//! Rows are padded to 128-bit boundaries for efficient word-based operations.
//! `(x, y)` and linear indexing like a dense `Vec<bool>` 2D grid, but bit-packed.
//!
//! **Reach for this on per-pixel masks, not per-item ones.** The 8x pays when the mask is
//! frame-sized and the saving is memory traffic — cosmic-ray holds three at once, 14 MB packed
//! against 113 MB. It is a straight loss on the masks that count stars or point matches: at 10⁴
//! elements the packed and unpacked forms both fit L1, so packing recovers no traffic and only adds
//! a shift and a mask to every read. Measured at +49% on star-detection's O(n²) deduplication and
//! +82% on registration's fill-scatter-scan, which is why those stay `Vec<bool>`.

use std::ops::Index;

use crate::buffer_pool::PooledBuffer;
use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;

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

fn bit_layout(size: Size2us) -> BitLayout {
    if size.width == 0 || size.height == 0 {
        return BitLayout {
            stride: 0,
            len: 0,
            num_words: 0,
        };
    }

    let stride = size
        .width
        .div_ceil(ROW_ALIGNMENT_BITS)
        .checked_mul(ROW_ALIGNMENT_BITS)
        .expect("BitBuffer2 row stride overflow");
    let len = size
        .width
        .checked_mul(size.height)
        .expect("BitBuffer2 dimensions overflow");
    let total_bits = stride
        .checked_mul(size.height)
        .expect("BitBuffer2 storage size overflow");
    debug_assert_eq!(total_bits % BITS_PER_WORD, 0);
    BitLayout {
        stride,
        len,
        num_words: total_bits / BITS_PER_WORD,
    }
}

/// A 2D buffer storing boolean values packed as bits: `u64` words, rows padded to 128 bits.
///
/// **Padding invariant — readers mask, writers need not.** The bits between `width` and `stride`
/// in each row carry no data and are held at no particular value: [`Self::new_filled`] and
/// [`Self::fill`] set them along with everything else. Position-based access is therefore always
/// safe, and [`Self::count_ones`] masks them off per row. Code that reaches into [`Self::words`]
/// directly owns that masking itself — or must establish that its buffers have padding clear,
/// which [`Self::padding_is_clear`] states as a checkable precondition.
#[derive(Debug, Clone)]
pub(crate) struct BitBuffer2 {
    /// Packed bit storage, 64 values per word. Padding bits past each row's `width` are
    /// unspecified — see the type's invariant before reading these word-wise.
    pub(crate) words: Vec<u64>,
    pub(crate) size: Size2us,
    pub(crate) len: usize,
    /// Number of bits per row including padding (aligned to 128 bits).
    pub(crate) stride: usize,
}

impl PooledBuffer for BitBuffer2 {
    fn allocate(dimensions: Size2us) -> Self {
        Self::new_filled(dimensions, false)
    }

    fn dimensions(&self) -> Size2us {
        self.size
    }
}

impl BitBuffer2 {
    /// Create a new bit buffer filled with the given value.
    #[inline]
    pub(crate) fn new_filled(size: Size2us, value: bool) -> Self {
        let layout = bit_layout(size);
        let fill = if value { !0u64 } else { 0u64 };
        let words = vec![fill; layout.num_words];
        Self {
            words,
            size,
            len: layout.len,
            stride: layout.stride,
        }
    }

    /// Create a new bit buffer with all bits set to false.
    #[inline]
    pub(crate) fn new_default(size: Size2us) -> Self {
        Self::new_filled(size, false)
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
        self.get_at(self.size.point_of(idx))
    }

    /// Set a bit value at the given linear index (row-major, no padding).
    #[inline]
    pub(crate) fn set(&mut self, idx: usize, value: bool) {
        debug_assert!(idx < self.len);
        self.set_at(self.size.point_of(idx), value);
    }

    /// Get a bit value at the given (x, y) coordinates.
    #[inline]
    pub(crate) fn get_at(&self, pos: Vec2us) -> bool {
        debug_assert!(self.size.contains(pos));
        let bit_idx = pos.y * self.stride + pos.x;
        let word_idx = bit_idx / BITS_PER_WORD;
        let bit_in_word = bit_idx % BITS_PER_WORD;
        (self.words[word_idx] >> bit_in_word) & 1 != 0
    }

    /// Set a bit value at the given (x, y) coordinates.
    #[inline]
    pub(crate) fn set_at(&mut self, pos: Vec2us, value: bool) {
        debug_assert!(self.size.contains(pos));
        let bit_idx = pos.y * self.stride + pos.x;
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
        assert_eq!(self.size, other.size, "size mismatch");
        assert_eq!(self.stride, other.stride, "stride mismatch");
        self.words.copy_from_slice(&other.words);
    }

    /// Count the number of set bits (true values, excluding padding).
    #[inline]
    pub(crate) fn count_ones(&self) -> usize {
        if self.size.width == 0 || self.size.height == 0 {
            return 0;
        }

        let words_per_row = self.words_per_row();
        let bits_in_last_word = self.size.width % BITS_PER_WORD;
        // Number of fully-used words (all 64 bits valid)
        let full_words_per_row = self.size.width / BITS_PER_WORD;

        let mut count = 0usize;

        for y in 0..self.size.height {
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

    /// Whether every row's padding bits are zero.
    ///
    /// Word-wise code that counts bits without masking — `(a & !b).count_ones()` — is correct only
    /// on buffers where this holds. `new_default` and `from_predicate` produce them, and
    /// position-based writes preserve them; `fill(true)` and `new_filled(_, true)` do not. This
    /// scans the whole buffer, so it belongs in a `debug_assert!` and never on a release path.
    pub(crate) fn padding_is_clear(&self) -> bool {
        if self.size.width == 0 || self.size.height == 0 {
            return true;
        }
        let words_per_row = self.words_per_row();
        let full_words_per_row = self.size.width / BITS_PER_WORD;
        let bits_in_last_word = self.size.width % BITS_PER_WORD;
        (0..self.size.height).all(|y| {
            let row = &self.words[y * words_per_row..(y + 1) * words_per_row];
            let partial_is_clear =
                bits_in_last_word == 0 || row[full_words_per_row] >> bits_in_last_word == 0;
            let whole_padding_words = full_words_per_row + usize::from(bits_in_last_word != 0);
            partial_is_clear && row[whole_padding_words..].iter().all(|&word| word == 0)
        })
    }

    /// Build a mask by testing every pixel, accumulating a whole word before storing it.
    ///
    /// `predicate` takes the linear pixel index, the same one [`Self::get`] takes. This exists so
    /// callers converting a `Vec<bool>` do not fall into `set()` per pixel, which is a
    /// read-modify-write on a word and slower than the byte store it replaces.
    pub(crate) fn from_predicate(size: Size2us, predicate: impl Fn(usize) -> bool) -> Self {
        let mut buffer = Self::new_default(size);
        let words_per_row = buffer.words_per_row();
        for y in 0..size.height {
            let row_start = y * words_per_row;
            let row_base = y * size.width;
            for x0 in (0..size.width).step_by(BITS_PER_WORD) {
                let mut word = 0u64;
                for bit in 0..BITS_PER_WORD.min(size.width - x0) {
                    if predicate(row_base + x0 + bit) {
                        word |= 1u64 << bit;
                    }
                }
                buffer.words[row_start + x0 / BITS_PER_WORD] = word;
            }
        }
        buffer
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
        if self.get_at(Vec2us::new(x, y)) {
            &true
        } else {
            &false
        }
    }
}

// Note: IndexMut cannot be implemented for bit-packed storage because we cannot
// return a mutable reference to a single bit. Use `set(idx, value)` or
// `set_at(pos, value)` methods instead.

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
mod internals {
    use crate::bit_buffer2::{BitBuffer2, bit_layout};
    use crate::math::size2us::Size2us;

    impl BitBuffer2 {
        pub(crate) fn from_slice(size: Size2us, data: &[bool]) -> Self {
            let layout = bit_layout(size);
            assert_eq!(
                data.len(),
                layout.len,
                "data length {} does not match dimensions {}x{}={}",
                data.len(),
                size.width,
                size.height,
                layout.len
            );

            let mut buffer = Self::new_default(size);
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
