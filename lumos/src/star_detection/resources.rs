//! Reusable resources for star detection.
//!
//! The resources retain image buffers and stage-specific workspaces across detections.

use crate::bit_buffer2::BitBuffer2;
use crate::buffer_pool::BufferPool;
use crate::concurrency::job_scratch_pool::JobScratchPool;
use crate::math::size2us::Size2us;
use crate::star_detection::background::workspace::BackgroundWorkspace;
use crate::star_detection::convolution::FilterKernels;
use crate::star_detection::deblend::deblend_buffers::DeblendBuffers;
use crate::star_detection::detector::stages::filter::DuplicateScratch;
use crate::star_detection::labeling::labeler::Labeler;
use imaginarium::Buffer2;

/// Reusable buffers and stage workspaces for star detection.
///
/// Buffers are stored and reused across multiple `detect()` calls to avoid
/// allocation overhead. All buffers in the pool have the same dimensions.
///
/// This recycles planes between the *sequential* stages of one detection, which is why
/// `acquire_*`/`release_*` take `&mut self` — unlike
/// [`JobScratchPool`](crate::concurrency::job_scratch_pool::JobScratchPool) it never hands scratch to concurrent
/// jobs. A stage acquires its planes, is free to work them across rayon workers itself, and
/// releases them before the next stage runs.
///
/// `acquire_*` returns buffers with **unspecified contents**: a freshly allocated buffer is
/// zeroed, but a reused one keeps its previous data. Callers must overwrite before reading.
///
/// The two accessor pairs stay written out rather than collapsing into one generic
/// `acquire::<B>()` over [`BufferPool`]: the pools differ only in element type, so selecting one
/// generically would need a trait mapping the type back to its field, and a turbofish at each of
/// the ~35 call sites, to save four one-line methods.
#[derive(Debug)]
pub(crate) struct DetectionResources {
    pub(crate) dimensions: Size2us,
    /// Grayscale, scratch, background, noise — anything one f32 plane wide.
    floats: BufferPool<Buffer2<f32>>,
    /// Threshold masks, dilation scratch.
    bitmasks: BufferPool<BitBuffer2>,
    pub(crate) labeler: Labeler,
    pub(crate) background: BackgroundWorkspace,
    /// The deblenders' working sets, one per rayon fold split.
    pub(crate) deblend: JobScratchPool<DeblendBuffers>,
    /// The per-tile and per-star values a detection's medians sort, refilled for each.
    pub(crate) values: Vec<f32>,
    /// Their absolute deviations from the median, for a MAD that keeps the values.
    pub(crate) deviations: Vec<f32>,
    pub(crate) duplicates: DuplicateScratch,
    pub(crate) kernels: FilterKernels,
}

impl DetectionResources {
    /// Create resources for the given image dimensions.
    pub(crate) fn new(dimensions: Size2us) -> Self {
        Self {
            dimensions,
            floats: BufferPool::default(),
            bitmasks: BufferPool::default(),
            labeler: Labeler::default(),
            background: BackgroundWorkspace::default(),
            deblend: JobScratchPool::default(),
            values: Vec::new(),
            deviations: Vec::new(),
            duplicates: DuplicateScratch::default(),
            kernels: FilterKernels::default(),
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

    /// Acquire a `BitBuffer2` from the pool, or allocate a new one.
    pub(crate) fn acquire_bit(&mut self) -> BitBuffer2 {
        self.bitmasks.acquire(self.dimensions)
    }

    /// Return a `BitBuffer2` to the pool for reuse. It must have the pool's dimensions.
    pub(crate) fn release_bit(&mut self, buffer: BitBuffer2) {
        self.bitmasks.release(buffer, self.dimensions);
    }

    /// Clear all pooled buffers, freeing memory.
    pub(crate) fn clear(&mut self) {
        self.floats.clear();
        self.bitmasks.clear();
        self.labeler = Labeler::default();
        self.background.clear();
        self.deblend = JobScratchPool::default();
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
    use crate::star_detection::resources::DetectionResources;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) struct BufferCounts {
        pub floats: usize,
        pub bitmasks: usize,
    }

    /// The pools' occupancy, for readers outside this module — `dimensions` is already
    /// `pub(crate)` and needs no accessor, but the pools themselves are private.
    pub(crate) fn buffer_counts(resources: &DetectionResources) -> BufferCounts {
        BufferCounts {
            floats: pooled_count(&resources.floats),
            bitmasks: pooled_count(&resources.bitmasks),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::math::size2us::Size2us;
    use crate::star_detection::resources::DetectionResources;
    use crate::star_detection::resources::internals::BufferCounts;
    use crate::star_detection::resources::internals::buffer_counts;
    use imaginarium::Buffer2;

    #[test]
    fn pool_creation() {
        let pool = DetectionResources::new(Size2us::new(100, 50));
        assert_eq!(pool.dimensions, Size2us::new(100, 50));
        // Nothing is pooled up front, so the first acquire of each kind allocates.
        assert_eq!(
            buffer_counts(&pool),
            BufferCounts {
                floats: 0,
                bitmasks: 0,
            }
        );
    }

    /// Each kind of buffer: an acquire from an empty pool allocates, a release pools it, the next
    /// acquire hands back that same allocation, and one past it allocates again — the counts after
    /// every step, the reuse by address.
    #[test]
    fn acquire_reuses_what_release_returned() {
        let counts = |floats, bitmasks| BufferCounts { floats, bitmasks };
        let mut pool = DetectionResources::new(Size2us::new(64, 32));

        let first = pool.acquire_f32();
        assert_eq!((first.width(), first.height()), (64, 32));
        let address = first.as_ptr();
        pool.release_f32(first);
        assert_eq!(buffer_counts(&pool), counts(1, 0));
        let again = pool.acquire_f32();
        assert_eq!(again.as_ptr(), address);
        let fresh = pool.acquire_f32();
        assert_ne!(fresh.as_ptr(), address);
        assert_eq!(buffer_counts(&pool), counts(0, 0));
        pool.release_f32(again);
        pool.release_f32(fresh);
        assert_eq!(buffer_counts(&pool), counts(2, 0));

        let first = pool.acquire_bit();
        assert_eq!(first.size, Size2us::new(64, 32));
        let address = first.words.as_ptr();
        pool.release_bit(first);
        assert_eq!(buffer_counts(&pool), counts(2, 1));
        let again = pool.acquire_bit();
        assert_eq!(again.words.as_ptr(), address);
        let fresh = pool.acquire_bit();
        assert_ne!(fresh.words.as_ptr(), address);
        pool.release_bit(again);
        pool.release_bit(fresh);
        assert_eq!(buffer_counts(&pool), counts(2, 2));
    }

    #[test]
    fn pool_clear() {
        let mut pool = DetectionResources::new(Size2us::new(64, 64));

        let buf1 = pool.acquire_f32();
        let buf2 = pool.acquire_bit();

        pool.release_f32(buf1);
        pool.release_bit(buf2);
        assert_eq!(
            buffer_counts(&pool),
            BufferCounts {
                floats: 1,
                bitmasks: 1,
            }
        );

        pool.clear();
        assert_eq!(
            buffer_counts(&pool),
            BufferCounts {
                floats: 0,
                bitmasks: 0,
            }
        );
    }

    #[test]
    #[should_panic(expected = "assertion")]
    fn release_f32_wrong_dimensions_panics() {
        // A mismatched buffer must be rejected even in release builds: downstream kernels index
        // off the pool's declared dimensions, so a silently accepted mismatch would be a wrong
        // image.
        let mut pool = DetectionResources::new(Size2us::new(64, 64));
        let wrong_size = Buffer2::new_default(32, 32);
        pool.release_f32(wrong_size);
    }

    #[test]
    fn pool_reset() {
        let mut pool = DetectionResources::new(Size2us::new(64, 64));

        let buf = pool.acquire_f32();
        pool.release_f32(buf);

        pool.reset(Size2us::new(64, 64));
        assert_eq!(buffer_counts(&pool).floats, 1);

        pool.reset(Size2us::new(128, 128));
        assert_eq!(pool.dimensions, Size2us::new(128, 128));
        assert_eq!(buffer_counts(&pool).floats, 0);
    }
}
