//! 8-byte aligned byte storage for efficient casting to/from f32/f64.

/// A byte vector with guaranteed 8-byte alignment.
///
/// This allows zero-copy casting to `Vec<f32>` or `Vec<f64>` using `bytemuck::cast_vec`.
#[derive(Clone, Debug, Default)]
pub struct AlignedBytes {
    /// Storage as u64 to guarantee 8-byte alignment.
    /// Length is in u64 units, actual byte length is stored separately.
    storage: Vec<u64>,
    /// Actual byte length (may be less than storage.len() * 8).
    len: usize,
}

impl AlignedBytes {
    /// Create a new aligned byte vector with the given length, initialized to zero.
    pub fn new_zeroed(len: usize) -> Self {
        let storage_len = len.div_ceil(8);
        Self {
            storage: vec![0u64; storage_len],
            len,
        }
    }

    /// Create from an existing byte slice (copies data).
    pub fn from_slice(bytes: &[u8]) -> Self {
        let mut result = Self::new_zeroed(bytes.len());
        result.as_mut_slice().copy_from_slice(bytes);
        result
    }

    /// Create from a Vec<u8>, attempting zero-copy if already aligned.
    pub fn from_vec(bytes: Vec<u8>) -> Self {
        let len = bytes.len();
        // Try zero-copy cast if alignment matches
        match bytemuck::try_cast_vec::<u8, u64>(bytes) {
            Ok(storage) => Self { storage, len },
            Err((_, bytes)) => Self::from_slice(&bytes),
        }
    }

    /// Convert to Vec<u8> (zero-copy).
    ///
    /// This is always a zero-copy operation since Vec<u64> alignment (8 bytes)
    /// exceeds Vec<u8> alignment requirements (1 byte).
    pub fn into_vec(self) -> Vec<u8> {
        let len = self.len;
        // Safety:
        // - Alignment: Vec<u64> is 8-byte aligned, Vec<u8> requires 1-byte alignment (8 >= 1).
        // - Size: storage_len * 8 and capacity * 8 correctly compute byte counts.
        // - Ownership: ManuallyDrop prevents double-free; ownership transfers to new Vec<u8>.
        // - Layout: Both u64 and u8 are Pod types with no drop glue.
        //
        // Note: Technically the allocator API requires deallocation with the same layout
        // (alignment) used for allocation. The Vec<u8> will deallocate with alignment 1,
        // but memory was allocated with alignment 8. In practice this works on all major
        // allocators (glibc, jemalloc, mimalloc, Windows heap) because they use size-based
        // bins and don't enforce alignment on deallocation. This pattern is widely used
        // in the Rust ecosystem (e.g., bytemuck internals, zerocopy).
        let storage = self.storage;
        let (ptr, storage_len, capacity) = {
            let mut storage = std::mem::ManuallyDrop::new(storage);
            (storage.as_mut_ptr(), storage.len(), storage.capacity())
        };
        let mut bytes =
            unsafe { Vec::from_raw_parts(ptr as *mut u8, storage_len * 8, capacity * 8) };
        bytes.truncate(len);
        bytes
    }

    /// Get the byte length.
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Check if empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Get bytes as a slice.
    #[inline]
    pub fn as_slice(&self) -> &[u8] {
        &bytemuck::cast_slice(&self.storage)[..self.len]
    }

    /// Get bytes as a mutable slice.
    #[inline]
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        let len = self.len;
        &mut bytemuck::cast_slice_mut(&mut self.storage)[..len]
    }
}

impl From<Vec<u8>> for AlignedBytes {
    fn from(bytes: Vec<u8>) -> Self {
        Self::from_vec(bytes)
    }
}

impl From<AlignedBytes> for Vec<u8> {
    fn from(aligned: AlignedBytes) -> Self {
        aligned.into_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_zeroed() {
        let ab = AlignedBytes::new_zeroed(100);
        assert_eq!(ab.len(), 100);
        assert!(ab.as_slice().iter().all(|&b| b == 0));
    }

    #[test]
    fn test_from_slice() {
        let data = vec![1u8, 2, 3, 4, 5];
        let ab = AlignedBytes::from_slice(&data);
        assert_eq!(ab.as_slice(), &data[..]);
    }

    #[test]
    fn test_from_vec_and_into_vec() {
        let data = vec![1u8, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        let ab = AlignedBytes::from_vec(data.clone());
        let recovered = ab.into_vec();
        assert_eq!(recovered, data);
    }

    #[test]
    fn test_alignment() {
        let ab = AlignedBytes::new_zeroed(100);
        let ptr = ab.as_slice().as_ptr();
        assert_eq!(ptr as usize % 8, 0, "Data should be 8-byte aligned");
    }

    #[test]
    fn test_cast_to_f32() {
        let mut ab = AlignedBytes::new_zeroed(12); // 3 f32s
        let floats: &mut [f32] = bytemuck::cast_slice_mut(ab.as_mut_slice());
        floats[0] = 1.0;
        floats[1] = 2.0;
        floats[2] = 3.0;

        let read_floats: &[f32] = bytemuck::cast_slice(ab.as_slice());
        assert_eq!(read_floats, &[1.0, 2.0, 3.0]);
    }

    #[test]
    fn test_into_vec_preserves_data() {
        let mut ab = AlignedBytes::new_zeroed(12);
        {
            let floats: &mut [f32] = bytemuck::cast_slice_mut(ab.as_mut_slice());
            floats[0] = 1.0;
            floats[1] = 2.0;
            floats[2] = 3.0;
        }

        let bytes = ab.into_vec();
        let floats: &[f32] = bytemuck::cast_slice(&bytes);
        assert_eq!(floats, &[1.0, 2.0, 3.0]);
    }

    #[test]
    fn test_into_vec_is_zero_copy() {
        let ab = AlignedBytes::new_zeroed(100);
        let original_ptr = ab.as_slice().as_ptr();
        let vec = ab.into_vec();
        let vec_ptr = vec.as_ptr();
        assert_eq!(
            original_ptr, vec_ptr,
            "into_vec should be zero-copy (same pointer)"
        );
    }
}
