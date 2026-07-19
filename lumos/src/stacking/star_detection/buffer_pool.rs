//! Buffer pool for reusing allocations across multiple star detections.
//!
//! This module provides a pool of reusable buffers to avoid repeated allocations
//! when processing multiple images of the same dimensions.

use common::BitBuffer2;
use common::Vec2us;
use imaginarium::Buffer2;

/// Pool of reusable buffers for star detection.
///
/// Buffers are stored and reused across multiple `detect()` calls to avoid
/// allocation overhead. All buffers in the pool have the same dimensions.
///
/// `acquire_*` returns buffers with **unspecified contents**: a freshly allocated buffer is
/// zeroed, but a reused one keeps its previous data. Callers must overwrite before reading.
#[derive(Debug)]
pub(crate) struct BufferPool {
    dimensions: Vec2us,
    /// Pool of f32 buffers (for grayscale, scratch, background, noise, etc.)
    f32_buffers: Vec<Buffer2<f32>>,
    /// Pool of BitBuffer2 (for threshold masks, dilation scratch, etc.)
    bit_buffers: Vec<BitBuffer2>,
    /// Single u32 buffer for label map (only one needed at a time)
    u32_buffer: Option<Buffer2<u32>>,
}

impl BufferPool {
    /// Create a new buffer pool for the given image dimensions.
    pub(crate) fn new(width: usize, height: usize) -> Self {
        Self {
            dimensions: Vec2us::new(width, height),
            f32_buffers: Vec::new(),
            bit_buffers: Vec::new(),
            u32_buffer: None,
        }
    }

    /// Get the expected width for buffers in this pool.
    #[inline]
    pub(crate) fn width(&self) -> usize {
        self.dimensions.x
    }

    /// Get the expected height for buffers in this pool.
    #[inline]
    pub(crate) fn height(&self) -> usize {
        self.dimensions.y
    }

    /// Acquire an f32 buffer from the pool, or allocate a new one.
    pub(crate) fn acquire_f32(&mut self) -> Buffer2<f32> {
        self.f32_buffers
            .pop()
            .unwrap_or_else(|| Buffer2::new_default(self.dimensions.x, self.dimensions.y))
    }

    /// Return an f32 buffer to the pool for reuse.
    ///
    /// The buffer must have the correct dimensions.
    pub(crate) fn release_f32(&mut self, buffer: Buffer2<f32>) {
        // Release asserts, not debug: a mismatched buffer handed back to acquire_f32() would be
        // silently reused by SIMD kernels elsewhere in this module that do unchecked-length
        // loads/stores off the pool's declared dimensions — out-of-bounds UB, not a wrong pixel.
        // The check is O(1) per acquire/release, not "too expensive for release".
        assert_eq!(buffer.width(), self.dimensions.x);
        assert_eq!(buffer.height(), self.dimensions.y);
        self.f32_buffers.push(buffer);
    }

    /// Acquire a BitBuffer2 from the pool, or allocate a new one.
    pub(crate) fn acquire_bit(&mut self) -> BitBuffer2 {
        self.bit_buffers
            .pop()
            .unwrap_or_else(|| BitBuffer2::new_filled(self.dimensions.x, self.dimensions.y, false))
    }

    /// Return a BitBuffer2 to the pool for reuse.
    ///
    /// The buffer must have the correct dimensions.
    pub(crate) fn release_bit(&mut self, buffer: BitBuffer2) {
        // See release_f32: release assert, not debug — a mismatch here is out-of-bounds UB in
        // a downstream SIMD kernel, not a wrong pixel.
        assert_eq!(buffer.width(), self.dimensions.x);
        assert_eq!(buffer.height(), self.dimensions.y);
        self.bit_buffers.push(buffer);
    }

    /// Acquire the u32 buffer (for label map), or allocate a new one.
    pub(crate) fn acquire_u32(&mut self) -> Buffer2<u32> {
        self.u32_buffer
            .take()
            .unwrap_or_else(|| Buffer2::new_default(self.dimensions.x, self.dimensions.y))
    }

    /// Return the u32 buffer to the pool for reuse.
    ///
    /// The buffer must have the correct dimensions.
    pub(crate) fn release_u32(&mut self, buffer: Buffer2<u32>) {
        // See release_f32: release assert, not debug — a mismatch here is out-of-bounds UB in
        // a downstream SIMD kernel, not a wrong pixel.
        assert_eq!(buffer.width(), self.dimensions.x);
        assert_eq!(buffer.height(), self.dimensions.y);
        self.u32_buffer = Some(buffer);
    }

    /// Clear all pooled buffers, freeing memory.
    pub(crate) fn clear(&mut self) {
        self.f32_buffers.clear();
        self.bit_buffers.clear();
        self.u32_buffer = None;
    }

    /// Reset the pool for new dimensions, clearing all buffers.
    pub(crate) fn reset(&mut self, width: usize, height: usize) {
        if self.dimensions.x != width || self.dimensions.y != height {
            self.clear();
            self.dimensions.x = width;
            self.dimensions.y = height;
        }
    }
}

/// Snapshot of how many buffers the pool currently holds, per kind. Memory tests read it to assert
/// the detection working set stays flat across detections (no per-frame buffer leak).
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PoolCounts {
    pub floats: usize,
    pub bitmasks: usize,
    pub labels: usize,
}

/// Dimension check and pool-size introspection used only by tests to assert pool sizing/reuse.
#[cfg(test)]
impl BufferPool {
    pub(crate) fn matches_dimensions(&self, width: usize, height: usize) -> bool {
        self.dimensions.x == width && self.dimensions.y == height
    }

    pub(crate) fn counts(&self) -> PoolCounts {
        PoolCounts {
            floats: self.f32_buffers.len(),
            bitmasks: self.bit_buffers.len(),
            labels: self.u32_buffer.is_some() as usize,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::stacking::star_detection::buffer_pool::*;

    #[test]
    fn test_pool_creation() {
        let pool = BufferPool::new(100, 50);
        assert_eq!(pool.width(), 100);
        assert_eq!(pool.height(), 50);
        assert!(pool.matches_dimensions(100, 50));
        assert!(!pool.matches_dimensions(50, 100));
    }

    #[test]
    fn test_f32_buffer_acquire_release() {
        let mut pool = BufferPool::new(64, 64);

        // First acquire allocates
        let buf1 = pool.acquire_f32();
        assert_eq!(buf1.width(), 64);
        assert_eq!(buf1.height(), 64);

        // Release returns to pool
        pool.release_f32(buf1);

        // Second acquire reuses
        let buf2 = pool.acquire_f32();
        assert_eq!(buf2.width(), 64);

        // Third acquire allocates new
        let buf3 = pool.acquire_f32();
        assert_eq!(buf3.width(), 64);

        pool.release_f32(buf2);
        pool.release_f32(buf3);
    }

    #[test]
    fn test_bit_buffer_acquire_release() {
        let mut pool = BufferPool::new(128, 64);

        let buf1 = pool.acquire_bit();
        assert_eq!(buf1.width(), 128);
        assert_eq!(buf1.height(), 64);

        pool.release_bit(buf1);

        let buf2 = pool.acquire_bit();
        assert_eq!(buf2.width(), 128);

        pool.release_bit(buf2);
    }

    #[test]
    fn test_u32_buffer_acquire_release() {
        let mut pool = BufferPool::new(32, 32);

        let buf1 = pool.acquire_u32();
        assert_eq!(buf1.width(), 32);
        assert_eq!(buf1.height(), 32);

        pool.release_u32(buf1);

        // Second acquire reuses same buffer
        let buf2 = pool.acquire_u32();
        assert_eq!(buf2.width(), 32);

        pool.release_u32(buf2);
    }

    #[test]
    fn test_pool_clear() {
        let mut pool = BufferPool::new(64, 64);

        let buf1 = pool.acquire_f32();
        let buf2 = pool.acquire_bit();
        let buf3 = pool.acquire_u32();

        pool.release_f32(buf1);
        pool.release_bit(buf2);
        pool.release_u32(buf3);

        pool.clear();

        // After clear, new acquires allocate fresh buffers
        let _ = pool.acquire_f32();
        let _ = pool.acquire_bit();
        let _ = pool.acquire_u32();
    }

    #[test]
    #[should_panic(expected = "assertion")]
    fn test_release_f32_wrong_dimensions_panics() {
        // A mismatched buffer must be rejected even in release builds: downstream SIMD kernels
        // do unchecked-length loads/stores off the pool's declared dimensions, so a silently
        // accepted mismatch would be out-of-bounds UB, not just a wrong pixel.
        let mut pool = BufferPool::new(64, 64);
        let wrong_size = Buffer2::new_default(32, 32);
        pool.release_f32(wrong_size);
    }

    #[test]
    fn test_pool_reset() {
        let mut pool = BufferPool::new(64, 64);

        let buf = pool.acquire_f32();
        pool.release_f32(buf);

        // Reset to same dimensions keeps buffers
        pool.reset(64, 64);
        assert_eq!(pool.f32_buffers.len(), 1);

        // Reset to different dimensions clears buffers
        pool.reset(128, 128);
        assert_eq!(pool.width(), 128);
        assert_eq!(pool.height(), 128);
        assert!(pool.f32_buffers.is_empty());
    }
}
