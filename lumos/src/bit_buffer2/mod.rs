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

mod dilation;

use std::ops::Index;

use rayon::prelude::*;
use rayon::slice::ChunksExactMut;

use crate::math::size2us::Size2us;
use crate::math::vec2us::Vec2us;
use crate::scratch_pool::Scratch;

/// Number of bits per storage word.
const BITS_PER_WORD: usize = 64;

/// Row alignment in bits (128 bits = 16 bytes = 2 words).
const ROW_ALIGNMENT_BITS: usize = 128;

/// A buffer's row stride in bits, and the words its rows take.
#[derive(Debug)]
struct BitLayout {
    stride: usize,
    num_words: usize,
}

impl BitLayout {
    const fn of(size: Size2us) -> Self {
        if size.width == 0 || size.height == 0 {
            return Self {
                stride: 0,
                num_words: 0,
            };
        }

        let stride = size
            .width
            .div_ceil(ROW_ALIGNMENT_BITS)
            .checked_mul(ROW_ALIGNMENT_BITS)
            .expect("BitBuffer2 row stride overflow");
        size.width
            .checked_mul(size.height)
            .expect("BitBuffer2 dimensions overflow");
        let total_bits = stride
            .checked_mul(size.height)
            .expect("BitBuffer2 storage size overflow");
        debug_assert!(total_bits.is_multiple_of(BITS_PER_WORD));
        Self {
            stride,
            num_words: total_bits / BITS_PER_WORD,
        }
    }
}

/// A 2D buffer storing boolean values packed as bits: `u64` words, rows padded to 128 bits.
///
/// **Padding invariant — the bits past each row's width are always clear.** Every constructor and
/// method keeps them so, which lets a word-wise reader count, compare, smear and visit whole words
/// with no mask. A writer through [`Self::row_words_mut`] or [`Self::par_rows_mut`] keeps them
/// clear too: the bits of a row's last pixel word outside [`Self::last_word_mask`], and the words
/// after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BitBuffer2 {
    /// Packed bit storage, 64 values per word, each row's padding clear.
    words: Vec<u64>,
    pub(crate) size: Size2us,
    /// Bits per row including padding: the width rounded up to 128.
    stride: usize,
}

/// A frame-sized mask: a value's shape is its size, and it is reused as it is.
impl Scratch for BitBuffer2 {
    type Shape = Size2us;

    fn build(shape: Size2us) -> Self {
        Self::new_filled(shape, false)
    }

    fn fit(&mut self, shape: Size2us) {
        debug_assert!(self.has_shape(shape));
    }

    fn has_shape(&self, shape: Size2us) -> bool {
        self.size == shape
    }
}

impl BitBuffer2 {
    #[inline]
    pub(crate) fn new_filled(size: Size2us, value: bool) -> Self {
        let layout = BitLayout::of(size);
        let mut buffer = Self {
            words: vec![0; layout.num_words],
            size,
            stride: layout.stride,
        };
        if value {
            buffer.fill(true);
        }
        buffer
    }

    /// The bytes a buffer of `size` allocates, rows padded and all.
    pub(crate) const fn heap_bytes(size: Size2us) -> usize {
        BitLayout::of(size).num_words * size_of::<u64>()
    }

    #[inline]
    pub(crate) fn new_default(size: Size2us) -> Self {
        Self::new_filled(size, false)
    }

    /// A buffer of no pixels, which allocates nothing: what [`Self::clear_to`] sizes.
    pub(crate) const fn empty() -> Self {
        Self {
            words: Vec::new(),
            size: Size2us::new(0, 0),
            stride: 0,
        }
    }

    /// Every bit clear at `size`, in the allocation this buffer already holds when it is large
    /// enough: a buffer reused for planes of several sizes grows once, to the largest.
    pub(crate) fn clear_to(&mut self, size: Size2us) {
        let layout = BitLayout::of(size);
        self.words.clear();
        self.words.resize(layout.num_words, 0);
        self.size = size;
        self.stride = layout.stride;
    }

    #[inline]
    pub(crate) const fn words_per_row(&self) -> usize {
        self.stride / BITS_PER_WORD
    }

    /// The words of row `y`, padding words included, which are 0.
    #[inline]
    pub(crate) fn row_words(&self, y: usize) -> &[u64] {
        let words_per_row = self.words_per_row();
        &self.words[y * words_per_row..(y + 1) * words_per_row]
    }

    /// The words of row `y`, for a writer that keeps the padding clear (see the type).
    #[inline]
    pub(crate) fn row_words_mut(&mut self, y: usize) -> &mut [u64] {
        let words_per_row = self.words_per_row();
        &mut self.words[y * words_per_row..(y + 1) * words_per_row]
    }

    /// Every row's words, row by row in parallel, for writers that keep the padding clear (see the
    /// type).
    pub(crate) fn par_rows_mut(&mut self) -> ChunksExactMut<'_, u64> {
        let words_per_row = self.words_per_row().max(1);
        self.words.par_chunks_exact_mut(words_per_row)
    }

    /// The bits of a row's last pixel word that hold pixels: all 64 when the width is a multiple
    /// of 64.
    #[inline]
    pub(crate) const fn last_word_mask(&self) -> u64 {
        match self.size.width % BITS_PER_WORD {
            0 => u64::MAX,
            bits => (1u64 << bits) - 1,
        }
    }

    /// Get a bit value at the given linear index (row-major, no padding).
    #[inline]
    pub(crate) fn get(&self, idx: usize) -> bool {
        self.get_at(self.size.point_of(idx))
    }

    /// Set a bit value at the given linear index (row-major, no padding).
    #[inline]
    pub(crate) fn set(&mut self, idx: usize, value: bool) {
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

    /// Every pixel set to `value`, the padding left clear.
    #[inline]
    pub(crate) fn fill(&mut self, value: bool) {
        if !value || self.words.is_empty() {
            self.words.fill(0);
            return;
        }
        let pixel_words = self.size.width.div_ceil(BITS_PER_WORD);
        let last = self.last_word_mask();
        for row in self.words.chunks_exact_mut(self.stride / BITS_PER_WORD) {
            row[..pixel_words].fill(u64::MAX);
            row[pixel_words - 1] = last;
            row[pixel_words..].fill(0);
        }
    }

    /// Count the number of set bits: whole words, as the padding holds none.
    #[inline]
    pub(crate) fn count_ones(&self) -> usize {
        self.words
            .iter()
            .map(|word| word.count_ones() as usize)
            .sum()
    }

    /// Fill the mask by testing every pixel, row by row in parallel, accumulating a whole word
    /// before storing it.
    ///
    /// `predicate` takes the linear pixel index, the same one [`Self::get`] takes. This exists so
    /// callers converting a `Vec<bool>` do not fall into `set()` per pixel, which is a
    /// read-modify-write on a word and slower than the byte store it replaces — and so a caller
    /// rebuilding the same mask every iteration can hand back the buffer it already has. A mask
    /// made from another mask is word-wise instead: [`Self::get`] divides the index back into a
    /// position.
    ///
    /// Every word is stored, padding included, so nothing the previous holder left remains.
    pub(crate) fn fill_from_predicate(&mut self, predicate: impl Fn(usize) -> bool + Sync) {
        let width = self.size.width;
        self.par_rows_mut().enumerate().for_each(|(y, row)| {
            let row_base = y * width;
            for (w, word) in row.iter_mut().enumerate() {
                let x0 = w * BITS_PER_WORD;
                let mut bits = 0u64;
                for bit in 0..BITS_PER_WORD.min(width.saturating_sub(x0)) {
                    if predicate(row_base + x0 + bit) {
                        bits |= 1u64 << bit;
                    }
                }
                *word = bits;
            }
        });
    }

    /// Set every bit `other` has set: `self |= other`, a word at a time.
    pub(crate) fn or_with(&mut self, other: &Self) {
        debug_assert_eq!(self.size, other.size, "size mismatch");
        for (word, &other) in self.words.iter_mut().zip(&other.words) {
            *word |= other;
        }
    }

    /// Clear every bit `other` has set: `self &= !other`, a word at a time.
    pub(crate) fn and_not(&mut self, other: &Self) {
        debug_assert_eq!(self.size, other.size, "size mismatch");
        for (word, &other) in self.words.iter_mut().zip(&other.words) {
            *word &= !other;
        }
    }

    /// Whether any pixel of the square `radius` either side of `centre`, clipped to the buffer, is
    /// set: word by word along each of its rows.
    pub(crate) fn any_in_square(&self, centre: Vec2us, radius: usize) -> bool {
        if self.size.width == 0 || self.size.height == 0 {
            return false;
        }
        let (x0, x1) = (
            centre.x.saturating_sub(radius),
            (centre.x + radius).min(self.size.width - 1),
        );
        let (y0, y1) = (
            centre.y.saturating_sub(radius),
            (centre.y + radius).min(self.size.height - 1),
        );
        (y0..=y1).any(|y| {
            let (first, last) = (y * self.stride + x0, y * self.stride + x1);
            (first / BITS_PER_WORD..=last / BITS_PER_WORD).any(|word| {
                let low = if word == first / BITS_PER_WORD {
                    first % BITS_PER_WORD
                } else {
                    0
                };
                let high = if word == last / BITS_PER_WORD {
                    last % BITS_PER_WORD
                } else {
                    BITS_PER_WORD - 1
                };
                let mask = (u64::MAX >> (BITS_PER_WORD - 1 - high)) & (u64::MAX << low);
                self.words[word] & mask != 0
            })
        })
    }

    /// Call `visit` with the position of every set bit, row by row, skipping each clear word whole
    /// — a sparse mask costs a pass over its words, not over its pixels.
    pub(crate) fn for_each_set(&self, mut visit: impl FnMut(Vec2us)) {
        for y in 0..self.size.height {
            for (word_in_row, &word) in self.row_words(y).iter().enumerate() {
                let base = word_in_row * BITS_PER_WORD;
                let mut bits = word;
                while bits != 0 {
                    visit(Vec2us::new(base + bits.trailing_zeros() as usize, y));
                    bits &= bits - 1;
                }
            }
        }
    }

    /// Dilate in place by a disk of `radius`: a pixel is set when a set pixel lies within Euclidean
    /// distance `radius` of it. `scratch`, of any contents and this buffer's size, holds the source
    /// while the buffer is rewritten.
    ///
    /// A disk rather than a square, as photutils dilates its source masks: a square reaches √2
    /// times further along the diagonals and masks sky that no source touches.
    ///
    /// `scratch` is this buffer's size, and `radius` at most 63.
    pub(crate) fn dilate(&mut self, radius: usize, scratch: &mut Self) {
        dilation::dilate_mask(self, radius, scratch);
    }
}

/// Index by linear index.
impl Index<usize> for BitBuffer2 {
    type Output = bool;

    #[inline]
    fn index(&self, idx: usize) -> &Self::Output {
        // A packed bit has no address to lend; the two promoted constants do.
        if self.get(idx) { &true } else { &false }
    }
}

#[cfg(test)]
mod internals {
    use imaginarium::Buffer2;

    use crate::bit_buffer2::BitBuffer2;
    use crate::math::size2us::Size2us;

    impl BitBuffer2 {
        /// Every bit in row-major order, padding skipped.
        pub(crate) fn iter(&self) -> impl Iterator<Item = bool> + '_ {
            (0..self.size.pixel_count()).map(|index| self.get(index))
        }

        /// The mask `data` holds, a bit a pixel.
        pub(crate) fn from_plane(data: &Buffer2<bool>) -> Self {
            let mut buffer = Self::new_default(Size2us::from(data));
            buffer.fill_from_predicate(|index| data[index]);
            buffer
        }

        /// Where the words are allocated, for a test that a buffer was reused.
        pub(crate) fn words_ptr(&self) -> *const u64 {
            self.words.as_ptr()
        }

        /// Whether every row's padding bits are zero, as the type holds them.
        pub(crate) fn padding_is_clear(&self) -> bool {
            if self.size.width == 0 {
                return true;
            }
            let pixel_words = self.size.width.div_ceil(64);
            let mask = self.last_word_mask();
            (0..self.size.height).all(|y| {
                let row = self.row_words(y);
                row[pixel_words - 1] & !mask == 0
                    && row[pixel_words..].iter().all(|&word| word == 0)
            })
        }
    }
}

#[cfg(feature = "internals")]
pub(crate) mod bench;
#[cfg(test)]
mod tests;
