//! Buffer pool for reusing allocations across multiple star detections.
//!
//! This module provides a pool of reusable buffers to avoid repeated allocations
//! when processing multiple images of the same dimensions.

use crate::common::{BitBuffer2, Buffer2};
use crate::star_detection::background::BackgroundMap;

/// Pool of reusable buffers for star detection.
///
/// Buffers are stored and reused across multiple `detect()` calls to avoid
/// allocation overhead. All buffers in the pool have the same dimensions.
#[derive(Debug)]
pub struct BufferPool {
    width: usize,
    height: usize,
    /// Pool of f32 buffers (for grayscale, scratch, background, noise, etc.)
    f32_buffers: Vec<Buffer2<f32>>,
    /// Pool of BitBuffer2 (for threshold masks, dilation scratch, etc.)
    bit_buffers: Vec<BitBuffer2>,
    /// Single u32 buffer for label map (only one needed at a time)
    u32_buffer: Option<Buffer2<u32>>,
    /// Pre-allocated BackgroundMap (reused across detections)
    background_map: Option<BackgroundMap>,
}

impl BufferPool {
    /// Create a new buffer pool for the given image dimensions.
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            f32_buffers: Vec::new(),
            bit_buffers: Vec::new(),
            u32_buffer: None,
            background_map: None,
        }
    }

    /// Get the expected width for buffers in this pool.
    #[inline]
    #[allow(dead_code)]
    pub fn width(&self) -> usize {
        self.width
    }

    /// Get the expected height for buffers in this pool.
    #[inline]
    #[allow(dead_code)]
    pub fn height(&self) -> usize {
        self.height
    }

    /// Check if the pool matches the given dimensions.
    #[inline]
    #[allow(dead_code)]
    pub fn matches_dimensions(&self, width: usize, height: usize) -> bool {
        self.width == width && self.height == height
    }

    /// Acquire an f32 buffer from the pool, or allocate a new one.
    pub fn acquire_f32(&mut self) -> Buffer2<f32> {
        self.f32_buffers
            .pop()
            .unwrap_or_else(|| Buffer2::new_default(self.width, self.height))
    }

    /// Return an f32 buffer to the pool for reuse.
    ///
    /// The buffer must have the correct dimensions.
    pub fn release_f32(&mut self, buffer: Buffer2<f32>) {
        debug_assert_eq!(buffer.width(), self.width);
        debug_assert_eq!(buffer.height(), self.height);
        self.f32_buffers.push(buffer);
    }

    /// Acquire a BitBuffer2 from the pool, or allocate a new one.
    pub fn acquire_bit(&mut self) -> BitBuffer2 {
        self.bit_buffers
            .pop()
            .unwrap_or_else(|| BitBuffer2::new_filled(self.width, self.height, false))
    }

    /// Return a BitBuffer2 to the pool for reuse.
    ///
    /// The buffer must have the correct dimensions.
    pub fn release_bit(&mut self, buffer: BitBuffer2) {
        debug_assert_eq!(buffer.width(), self.width);
        debug_assert_eq!(buffer.height(), self.height);
        self.bit_buffers.push(buffer);
    }

    /// Acquire the u32 buffer (for label map), or allocate a new one.
    #[allow(dead_code)]
    pub fn acquire_u32(&mut self) -> Buffer2<u32> {
        self.u32_buffer
            .take()
            .unwrap_or_else(|| Buffer2::new_default(self.width, self.height))
    }

    /// Return the u32 buffer to the pool for reuse.
    ///
    /// The buffer must have the correct dimensions.
    #[allow(dead_code)]
    pub fn release_u32(&mut self, buffer: Buffer2<u32>) {
        debug_assert_eq!(buffer.width(), self.width);
        debug_assert_eq!(buffer.height(), self.height);
        self.u32_buffer = Some(buffer);
    }

    /// Acquire a mutable reference to the background map, creating it if needed.
    ///
    /// The `with_adaptive` parameter indicates whether adaptive sigma is needed.
    pub fn acquire_background_map(&mut self, with_adaptive: bool) -> &mut BackgroundMap {
        self.background_map.get_or_insert_with(|| {
            BackgroundMap::new_uninit(self.width, self.height, with_adaptive)
        })
    }

    /// Get an immutable reference to the background map.
    ///
    /// Returns `None` if the background map hasn't been acquired yet.
    pub fn background_map(&self) -> Option<&BackgroundMap> {
        self.background_map.as_ref()
    }

    /// Clear all pooled buffers, freeing memory.
    pub fn clear(&mut self) {
        self.f32_buffers.clear();
        self.bit_buffers.clear();
        self.u32_buffer = None;
        self.background_map = None;
    }

    /// Reset the pool for new dimensions, clearing all buffers.
    pub fn reset(&mut self, width: usize, height: usize) {
        if self.width != width || self.height != height {
            self.clear();
            self.width = width;
            self.height = height;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
