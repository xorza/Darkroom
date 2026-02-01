//! Bit-packed 2D buffer for boolean masks.
//!
//! Uses 1 bit per element instead of 1 byte, reducing memory by 8x.
//! Rows are aligned to 128 bits (16 bytes) for efficient SIMD operations.
//! Memory is 16-byte aligned for optimal SIMD load/store operations.
//! Provides the same API as `Buffer2<bool>` for easy migration.

use aligned_vec::AVec;
use std::ops::Index;

/// Number of bits per storage word.
const BITS_PER_WORD: usize = 64;

/// Row alignment in bits (128 bits = 16 bytes = 2 words).
const ROW_ALIGNMENT_BITS: usize = 128;

/// Memory alignment in bytes (16 bytes for SIMD).
const ALIGNMENT: usize = 16;

/// A 2D buffer storing boolean values packed as bits.
///
/// Uses `u64` words internally, with 64 bits per word.
/// Rows are aligned to 128 bits (2 words) for efficient SIMD access.
/// Memory is 16-byte aligned for optimal SIMD load/store operations.
/// This reduces memory usage by 8x compared to `Vec<bool>`.
#[derive(Debug, Clone)]
pub struct BitBuffer2 {
    /// Packed bit storage. Each u64 holds 64 boolean values. 16-byte aligned.
    words: AVec<u64, aligned_vec::ConstAlign<ALIGNMENT>>,
    width: usize,
    height: usize,
    /// Number of bits per row including padding (aligned to 128 bits).
    stride: usize,
}

#[allow(dead_code)] // Public API - methods will be used as Buffer2<bool> is migrated
impl BitBuffer2 {
    /// Create a new bit buffer filled with the given value.
    #[inline]
    pub fn new_filled(width: usize, height: usize, value: bool) -> Self {
        // Align stride to 128 bits (2 words)
        let stride = width.div_ceil(ROW_ALIGNMENT_BITS) * ROW_ALIGNMENT_BITS;
        let total_bits = stride * height;
        let num_words = total_bits.div_ceil(BITS_PER_WORD);
        let fill = if value { !0u64 } else { 0u64 };
        let mut words = AVec::with_capacity(ALIGNMENT, num_words);
        words.resize(num_words, fill);
        Self {
            words,
            width,
            height,
            stride,
        }
    }

    /// Create a new bit buffer with all bits set to false.
    #[inline]
    pub fn new_default(width: usize, height: usize) -> Self {
        Self::new_filled(width, height, false)
    }

    /// Create a new bit buffer from a slice of booleans.
    ///
    /// The slice length must equal `width * height`.
    #[inline]
    pub fn from_slice(width: usize, height: usize, data: &[bool]) -> Self {
        let len = width * height;
        assert_eq!(
            data.len(),
            len,
            "data length {} does not match dimensions {}x{}={}",
            data.len(),
            width,
            height,
            len
        );

        let stride = width.div_ceil(ROW_ALIGNMENT_BITS) * ROW_ALIGNMENT_BITS;
        let total_bits = stride * height;
        let num_words = total_bits.div_ceil(BITS_PER_WORD);
        let mut words = AVec::with_capacity(ALIGNMENT, num_words);
        words.resize(num_words, 0u64);

        for y in 0..height {
            for x in 0..width {
                if data[y * width + x] {
                    let bit_idx = y * stride + x;
                    let word_idx = bit_idx / BITS_PER_WORD;
                    let bit_in_word = bit_idx % BITS_PER_WORD;
                    words[word_idx] |= 1u64 << bit_in_word;
                }
            }
        }

        Self {
            words,
            width,
            height,
            stride,
        }
    }

    /// Get the width of the buffer.
    #[inline]
    pub fn width(&self) -> usize {
        self.width
    }

    /// Get the height of the buffer.
    #[inline]
    pub fn height(&self) -> usize {
        self.height
    }

    /// Get the stride (bits per row including padding).
    #[inline]
    pub fn stride(&self) -> usize {
        self.stride
    }

    /// Get the number of words per row.
    #[inline]
    pub fn words_per_row(&self) -> usize {
        self.stride / BITS_PER_WORD
    }

    /// Get the total number of bits (width * height, excluding padding).
    #[inline]
    pub fn len(&self) -> usize {
        self.width * self.height
    }

    /// Check if the buffer is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// Get a bit value at the given linear index (row-major, no padding).
    #[inline]
    pub fn get(&self, idx: usize) -> bool {
        debug_assert!(idx < self.len());
        let x = idx % self.width;
        let y = idx / self.width;
        self.get_xy(x, y)
    }

    /// Set a bit value at the given linear index (row-major, no padding).
    #[inline]
    pub fn set(&mut self, idx: usize, value: bool) {
        debug_assert!(idx < self.len());
        let x = idx % self.width;
        let y = idx / self.width;
        self.set_xy(x, y, value);
    }

    /// Get a bit value at the given (x, y) coordinates.
    #[inline]
    pub fn get_xy(&self, x: usize, y: usize) -> bool {
        debug_assert!(x < self.width && y < self.height);
        let bit_idx = y * self.stride + x;
        let word_idx = bit_idx / BITS_PER_WORD;
        let bit_in_word = bit_idx % BITS_PER_WORD;
        (self.words[word_idx] >> bit_in_word) & 1 != 0
    }

    /// Set a bit value at the given (x, y) coordinates.
    #[inline]
    pub fn set_xy(&mut self, x: usize, y: usize, value: bool) {
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
    pub fn fill(&mut self, value: bool) {
        let fill = if value { !0u64 } else { 0u64 };
        self.words.fill(fill);
    }

    /// Get the underlying word storage.
    ///
    /// Each u64 contains 64 packed bits in LSB order.
    #[inline]
    pub fn words(&self) -> &[u64] {
        &self.words
    }

    /// Get mutable access to the underlying word storage.
    ///
    /// Each u64 contains 64 packed bits in LSB order.
    #[inline]
    pub fn words_mut(&mut self) -> &mut [u64] {
        &mut self.words
    }

    /// Get the number of u64 words in the storage.
    #[inline]
    pub fn num_words(&self) -> usize {
        self.words.len()
    }

    /// Copy contents from another BitBuffer2.
    #[inline]
    pub fn copy_from(&mut self, other: &Self) {
        assert_eq!(self.width, other.width, "width mismatch");
        assert_eq!(self.height, other.height, "height mismatch");
        assert_eq!(self.stride, other.stride, "stride mismatch");
        self.words.copy_from_slice(&other.words);
    }

    /// Swap contents with another BitBuffer2.
    #[inline]
    pub fn swap(&mut self, other: &mut Self) {
        assert_eq!(self.width, other.width, "width mismatch");
        assert_eq!(self.height, other.height, "height mismatch");
        assert_eq!(self.stride, other.stride, "stride mismatch");
        std::mem::swap(&mut self.words, &mut other.words);
    }

    /// Count the number of set bits (true values, excluding padding).
    #[inline]
    pub fn count_ones(&self) -> usize {
        let mut count = 0;
        for y in 0..self.height {
            for x in 0..self.width {
                if self.get_xy(x, y) {
                    count += 1;
                }
            }
        }
        count
    }

    /// Iterate over all bit values (row-major order, skipping padding).
    #[inline]
    pub fn iter(&self) -> BitIter<'_> {
        BitIter {
            buffer: self,
            x: 0,
            y: 0,
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
#[allow(dead_code)] // Public API - will be used as Buffer2<bool> is migrated
pub struct BitIter<'a> {
    buffer: &'a BitBuffer2,
    x: usize,
    y: usize,
}

impl Iterator for BitIter<'_> {
    type Item = bool;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        if self.y >= self.buffer.height {
            return None;
        }

        let value = self.buffer.get_xy(self.x, self.y);
        self.x += 1;
        if self.x >= self.buffer.width {
            self.x = 0;
            self.y += 1;
        }
        Some(value)
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.buffer.width * (self.buffer.height - self.y) - self.x;
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for BitIter<'_> {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_filled_false() {
        let buf = BitBuffer2::new_filled(100, 100, false);
        assert_eq!(buf.width(), 100);
        assert_eq!(buf.height(), 100);
        assert_eq!(buf.len(), 10000);
        for i in 0..buf.len() {
            assert!(!buf.get(i));
        }
    }

    #[test]
    fn test_new_filled_true() {
        let buf = BitBuffer2::new_filled(100, 100, true);
        for i in 0..buf.len() {
            assert!(buf.get(i));
        }
    }

    #[test]
    fn test_set_get() {
        let mut buf = BitBuffer2::new_filled(64, 64, false);

        buf.set(0, true);
        buf.set(63, true);
        buf.set(64, true);
        buf.set(127, true);

        assert!(buf.get(0));
        assert!(buf.get(63));
        assert!(buf.get(64));
        assert!(buf.get(127));
        assert!(!buf.get(1));
        assert!(!buf.get(62));
        assert!(!buf.get(65));
    }

    #[test]
    fn test_set_get_xy() {
        let mut buf = BitBuffer2::new_filled(100, 100, false);

        buf.set_xy(50, 50, true);
        buf.set_xy(0, 0, true);
        buf.set_xy(99, 99, true);

        assert!(buf.get_xy(50, 50));
        assert!(buf.get_xy(0, 0));
        assert!(buf.get_xy(99, 99));
        assert!(!buf.get_xy(50, 51));
    }

    #[test]
    fn test_index() {
        let mut buf = BitBuffer2::new_filled(100, 100, false);
        buf.set(42, true);

        assert!(buf[42]);
        assert!(!buf[41]);
        assert!(buf[(42, 0)]);
    }

    #[test]
    fn test_count_ones() {
        let mut buf = BitBuffer2::new_filled(100, 100, false);
        assert_eq!(buf.count_ones(), 0);

        buf.set(0, true);
        buf.set(100, true);
        buf.set(1000, true);
        assert_eq!(buf.count_ones(), 3);
    }

    #[test]
    fn test_fill() {
        let mut buf = BitBuffer2::new_filled(100, 100, false);
        buf.set(50, true);
        assert!(buf.get(50));

        buf.fill(false);
        assert!(!buf.get(50));

        buf.fill(true);
        for i in 0..buf.len() {
            assert!(buf.get(i));
        }
    }

    #[test]
    fn test_iter() {
        let mut buf = BitBuffer2::new_filled(10, 10, false);
        buf.set(5, true);
        buf.set(50, true);

        let values: Vec<bool> = buf.iter().collect();
        assert_eq!(values.len(), 100);
        assert!(values[5]);
        assert!(values[50]);
        assert!(!values[0]);
    }

    #[test]
    fn test_memory_size() {
        // 4096x4096: width already aligned to 128, so stride = 4096
        // Total bits = 4096 * 4096 = 16M bits = 256K words
        let buf = BitBuffer2::new_filled(4096, 4096, false);
        assert_eq!(buf.stride(), 4096);
        assert_eq!(buf.words_per_row(), 64); // 4096 / 64
        assert_eq!(buf.num_words(), 4096 * 64);
        // Memory: 256K * 8 bytes = 2MB (vs 16MB for Vec<bool>)
    }

    #[test]
    fn test_row_alignment() {
        // Width 100 should be aligned to 128 bits
        let buf = BitBuffer2::new_filled(100, 50, false);
        assert_eq!(buf.stride(), 128);
        assert_eq!(buf.words_per_row(), 2);

        // Width 200 should be aligned to 256 bits (next multiple of 128)
        let buf = BitBuffer2::new_filled(200, 50, false);
        assert_eq!(buf.stride(), 256);
        assert_eq!(buf.words_per_row(), 4);

        // Width 128 is already aligned
        let buf = BitBuffer2::new_filled(128, 50, false);
        assert_eq!(buf.stride(), 128);
        assert_eq!(buf.words_per_row(), 2);
    }

    #[test]
    fn test_memory_alignment() {
        // Verify that the underlying memory is 16-byte aligned
        let buf = BitBuffer2::new_filled(100, 100, false);
        let ptr = buf.words().as_ptr() as usize;
        assert_eq!(ptr % 16, 0, "Memory should be 16-byte aligned");

        let buf = BitBuffer2::from_slice(10, 10, &[false; 100]);
        let ptr = buf.words().as_ptr() as usize;
        assert_eq!(ptr % 16, 0, "Memory should be 16-byte aligned");
    }

    #[test]
    fn test_from_slice() {
        let data = vec![true, false, true, false, false, true];
        let buf = BitBuffer2::from_slice(3, 2, &data);

        assert_eq!(buf.width(), 3);
        assert_eq!(buf.height(), 2);
        assert_eq!(buf.len(), 6);

        assert!(buf.get(0));
        assert!(!buf.get(1));
        assert!(buf.get(2));
        assert!(!buf.get(3));
        assert!(!buf.get(4));
        assert!(buf.get(5));
    }

    #[test]
    fn test_from_slice_large() {
        // Test with more than 64 elements to cover multiple words
        let mut data = vec![false; 200];
        data[0] = true;
        data[63] = true;
        data[64] = true;
        data[127] = true;
        data[199] = true;

        let buf = BitBuffer2::from_slice(20, 10, &data);

        assert!(buf.get(0));
        assert!(buf.get(63));
        assert!(buf.get(64));
        assert!(buf.get(127));
        assert!(buf.get(199));
        assert!(!buf.get(1));
        assert!(!buf.get(100));
    }

    #[test]
    #[should_panic(expected = "data length")]
    fn test_from_slice_wrong_length() {
        let data = vec![true, false, true];
        BitBuffer2::from_slice(2, 2, &data); // expects 4, got 3
    }

    #[test]
    fn test_into_vec_bool() {
        let mut buf = BitBuffer2::new_filled(10, 10, false);
        buf.set(5, true);
        buf.set(50, true);
        buf.set(99, true);

        let vec: Vec<bool> = buf.into();

        assert_eq!(vec.len(), 100);
        assert!(vec[5]);
        assert!(vec[50]);
        assert!(vec[99]);
        assert!(!vec[0]);
        assert!(!vec[6]);
    }

    #[test]
    fn test_from_ref_to_vec_bool() {
        let mut buf = BitBuffer2::new_filled(5, 5, false);
        buf.set(0, true);
        buf.set(24, true);

        let vec: Vec<bool> = (&buf).into();

        assert_eq!(vec.len(), 25);
        assert!(vec[0]);
        assert!(vec[24]);
        assert!(!vec[1]);

        // Original buffer still usable
        assert!(buf.get(0));
    }

    #[test]
    fn test_roundtrip_slice_to_vec() {
        let original = vec![
            true, false, true, true, false, false, true, false, // 8
            false, true, false, true, true, false, false, true, // 16
        ];
        let buf = BitBuffer2::from_slice(4, 4, &original);
        let result: Vec<bool> = buf.into();

        assert_eq!(original, result);
    }
}
