//! Reusable resources for star detection.
//!
//! The resources retain image buffers and stage-specific workspaces across detections.

use crate::bit_buffer2::BitBuffer2;
use crate::buffer_pool::BufferPool;
use crate::math::size2us::Size2us;
use crate::stacking::star_detection::background::workspace::BackgroundWorkspace;
use imaginarium::Buffer2;

/// Reusable buffers and stage workspaces for star detection.
///
/// Buffers are stored and reused across multiple `detect()` calls to avoid
/// allocation overhead. All buffers in the pool have the same dimensions.
///
/// This recycles planes between the *sequential* stages of one detection, which is why
/// `acquire_*`/`release_*` take `&mut self` — unlike
/// [`JobScratchPool`](crate::concurrency::JobScratchPool) it never hands scratch to concurrent
/// jobs. A stage acquires its planes, is free to work them across rayon workers itself, and
/// releases them before the next stage runs.
///
/// `acquire_*` returns buffers with **unspecified contents**: a freshly allocated buffer is
/// zeroed, but a reused one keeps its previous data. Callers must overwrite before reading.
#[derive(Debug)]
pub(crate) struct DetectionResources {
    pub(crate) dimensions: Size2us,
    /// Grayscale, scratch, background, noise — anything one f32 plane wide.
    floats: BufferPool<Buffer2<f32>>,
    /// Threshold masks, dilation scratch.
    bitmasks: BufferPool<BitBuffer2>,
    /// Label maps. Only one is live at a time, but pooling rather than holding a single slot
    /// means a second release keeps both instead of dropping one on the floor.
    labels: BufferPool<Buffer2<u32>>,
    pub(crate) background: BackgroundWorkspace,
}

impl DetectionResources {
    /// Create resources for the given image dimensions.
    pub(crate) fn new(dimensions: Size2us) -> Self {
        Self {
            dimensions,
            floats: BufferPool::default(),
            bitmasks: BufferPool::default(),
            labels: BufferPool::default(),
            background: BackgroundWorkspace::default(),
        }
    }

    /// Acquire an f32 buffer from the pool, or allocate a new one.
    pub(crate) fn acquire_f32(&mut self) -> Buffer2<f32> {
        self.floats.acquire(self.dimensions)
    }

    /// Return an f32 buffer to the pool for reuse. It must have the pool's dimensions.
    pub(crate) fn release_f32(&mut self, buffer: Buffer2<f32>) {
        self.floats.release(buffer, self.dimensions);
    }

    /// Acquire a BitBuffer2 from the pool, or allocate a new one.
    pub(crate) fn acquire_bit(&mut self) -> BitBuffer2 {
        self.bitmasks.acquire(self.dimensions)
    }

    /// Return a BitBuffer2 to the pool for reuse. It must have the pool's dimensions.
    pub(crate) fn release_bit(&mut self, buffer: BitBuffer2) {
        self.bitmasks.release(buffer, self.dimensions);
    }

    /// Acquire a label map from the pool, or allocate a new one.
    pub(crate) fn acquire_u32(&mut self) -> Buffer2<u32> {
        self.labels.acquire(self.dimensions)
    }

    /// Return a label map to the pool for reuse. It must have the pool's dimensions.
    pub(crate) fn release_u32(&mut self, buffer: Buffer2<u32>) {
        self.labels.release(buffer, self.dimensions);
    }

    /// Clear all pooled buffers, freeing memory.
    pub(crate) fn clear(&mut self) {
        self.floats.clear();
        self.bitmasks.clear();
        self.labels.clear();
        self.background.clear();
    }

    /// Reset the pool for new dimensions, clearing all buffers.
    pub(crate) fn reset(&mut self, dimensions: Size2us) {
        if self.dimensions != dimensions {
            self.clear();
            self.dimensions = dimensions;
        }
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::buffer_pool::internals::pooled_count;
    use crate::math::size2us::Size2us;
    use crate::stacking::star_detection::resources::DetectionResources;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) struct BufferCounts {
        pub floats: usize,
        pub bitmasks: usize,
        pub labels: usize,
    }

    pub(crate) fn matches_dimensions(resources: &DetectionResources, dimensions: Size2us) -> bool {
        resources.dimensions == dimensions
    }

    pub(crate) fn buffer_counts(resources: &DetectionResources) -> BufferCounts {
        BufferCounts {
            floats: pooled_count(&resources.floats),
            bitmasks: pooled_count(&resources.bitmasks),
            labels: pooled_count(&resources.labels),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::math::size2us::Size2us;
    use crate::stacking::star_detection::resources::DetectionResources;
    use crate::stacking::star_detection::resources::internals::BufferCounts;
    use crate::stacking::star_detection::resources::internals::buffer_counts;
    use crate::stacking::star_detection::resources::internals::matches_dimensions;
    use imaginarium::Buffer2;

    #[test]
    fn test_pool_creation() {
        let pool = DetectionResources::new(Size2us::new(100, 50));
        assert_eq!(pool.dimensions, Size2us::new(100, 50));
        assert!(matches_dimensions(&pool, Size2us::new(100, 50)));
        assert!(!matches_dimensions(&pool, Size2us::new(50, 100)));
    }

    #[test]
    fn test_f32_buffer_acquire_release() {
        let mut pool = DetectionResources::new(Size2us::new(64, 64));

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
        let mut pool = DetectionResources::new(Size2us::new(128, 64));

        let buf1 = pool.acquire_bit();
        assert_eq!(buf1.size, Size2us::new(128, 64));

        pool.release_bit(buf1);

        let buf2 = pool.acquire_bit();
        assert_eq!(buf2.size.width, 128);

        pool.release_bit(buf2);
    }

    #[test]
    fn test_u32_buffer_acquire_release() {
        let mut pool = DetectionResources::new(Size2us::new(32, 32));

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
        let mut pool = DetectionResources::new(Size2us::new(64, 64));

        let buf1 = pool.acquire_f32();
        let buf2 = pool.acquire_bit();
        let buf3 = pool.acquire_u32();

        pool.release_f32(buf1);
        pool.release_bit(buf2);
        pool.release_u32(buf3);
        assert_eq!(
            buffer_counts(&pool),
            BufferCounts {
                floats: 1,
                bitmasks: 1,
                labels: 1,
            }
        );

        pool.clear();
        assert_eq!(
            buffer_counts(&pool),
            BufferCounts {
                floats: 0,
                bitmasks: 0,
                labels: 0,
            }
        );
    }

    #[test]
    #[should_panic(expected = "assertion")]
    fn test_release_f32_wrong_dimensions_panics() {
        // A mismatched buffer must be rejected even in release builds: downstream SIMD kernels
        // do unchecked-length loads/stores off the pool's declared dimensions, so a silently
        // accepted mismatch would be out-of-bounds UB, not just a wrong pixel.
        let mut pool = DetectionResources::new(Size2us::new(64, 64));
        let wrong_size = Buffer2::new_default(32, 32);
        pool.release_f32(wrong_size);
    }

    #[test]
    fn test_pool_reset() {
        let mut pool = DetectionResources::new(Size2us::new(64, 64));

        let buf = pool.acquire_f32();
        pool.release_f32(buf);

        // Reset to same dimensions keeps buffers
        pool.reset(Size2us::new(64, 64));
        assert_eq!(buffer_counts(&pool).floats, 1);

        // Reset to different dimensions clears buffers
        pool.reset(Size2us::new(128, 128));
        assert_eq!(pool.dimensions, Size2us::new(128, 128));
        assert_eq!(buffer_counts(&pool).floats, 0);
    }
}
