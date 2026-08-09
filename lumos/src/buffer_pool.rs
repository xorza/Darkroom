//! Dimension-checked reuse of image-sized buffers.
//!
//! The crate's other pool, [`JobScratchPool`](crate::concurrency::JobScratchPool), is not a
//! competing take on this one — the two differ in a way that forces everything else about their
//! APIs, and it is worth saying once here rather than rediscovering it:
//!
//! - `JobScratchPool` is `&self` behind a `Mutex` because it lends scratch to rayon workers
//!   running concurrently. This one is `&mut self`, serving the sequential stages of a single
//!   detection.
//! - That is why this one hands back *owned* buffers and takes them again by value, where the
//!   other returns an RAII lease. A lease here would borrow the pool, and a detection holds
//!   several planes live across stages — it could never hold two at once.
//! - This one allocates at a size, through [`PooledBuffer`]; the other only needs `T: Default`.
//!
//! What the two share is a free list and the rule below about acquired contents. Reach for
//! `JobScratchPool` when the consumers are concurrent, this one when they are stages.

use imaginarium::Buffer2;

use crate::math::size2us::Size2us;

/// An image-sized buffer a [`BufferPool`] can allocate on demand and dimension-check on return.
pub(crate) trait PooledBuffer: Sized {
    /// Allocate a buffer covering `dimensions`, zeroed or cleared as the type sees fit.
    fn allocate(dimensions: Size2us) -> Self;

    /// The dimensions this buffer actually covers.
    fn dimensions(&self) -> Size2us;
}

impl<T: Default + Clone> PooledBuffer for Buffer2<T> {
    fn allocate(dimensions: Size2us) -> Self {
        Buffer2::new_default(dimensions.width, dimensions.height)
    }

    fn dimensions(&self) -> Size2us {
        Size2us::new(self.width(), self.height())
    }
}

/// Buffers of one kind, all at the owner's dimensions, handed out and taken back.
///
/// The pool holds no dimensions of its own — the owner passes them in, so one owner's several
/// pools cannot disagree about what size they are pooling.
#[derive(Debug)]
pub(crate) struct BufferPool<B> {
    free: Vec<B>,
}

impl<B> Default for BufferPool<B> {
    fn default() -> Self {
        Self { free: Vec::new() }
    }
}

impl<B: PooledBuffer> BufferPool<B> {
    /// Take a buffer, or allocate one at `dimensions`.
    ///
    /// Contents are **unspecified**: a fresh buffer is zeroed, a reused one keeps whatever the
    /// last holder left. Overwrite before reading.
    ///
    /// Release assert, for the same reason as [`Self::release`]'s: handing out a buffer of the
    /// previous dimensions is out-of-bounds UB in the SIMD kernels downstream, not a wrong pixel.
    /// The two catch different mistakes — `release` catches a holder that resized a buffer,
    /// `acquire` a pool reused across image sizes without an intervening [`Self::clear`].
    pub(crate) fn acquire(&mut self, dimensions: Size2us) -> B {
        match self.free.pop() {
            Some(buffer) => {
                assert_eq!(buffer.dimensions(), dimensions);
                buffer
            }
            None => B::allocate(dimensions),
        }
    }

    /// Return a buffer for reuse. It must cover `dimensions`.
    ///
    /// Release assert, not debug: a mismatched buffer handed back would be silently reused by
    /// SIMD kernels that do unchecked-length loads and stores off the pool's declared dimensions
    /// — out-of-bounds UB, not a wrong pixel. The check is O(1) per acquire/release, not "too
    /// expensive for release".
    pub(crate) fn release(&mut self, buffer: B, dimensions: Size2us) {
        assert_eq!(buffer.dimensions(), dimensions);
        self.free.push(buffer);
    }

    /// Drop every pooled buffer, freeing their memory.
    pub(crate) fn clear(&mut self) {
        self.free.clear();
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::buffer_pool::BufferPool;

    /// How many buffers are pooled — once warm, the high-water mark of concurrent demand.
    pub(crate) fn pooled_count<B>(pool: &BufferPool<B>) -> usize {
        pool.free.len()
    }
}

#[cfg(test)]
mod tests {
    use crate::buffer_pool::BufferPool;
    use crate::buffer_pool::internals::pooled_count;
    use crate::math::size2us::Size2us;
    use imaginarium::Buffer2;

    const SMALL: Size2us = Size2us {
        width: 8,
        height: 4,
    };
    const LARGE: Size2us = Size2us {
        width: 16,
        height: 4,
    };

    #[test]
    fn a_pooled_buffer_comes_back_at_the_dimensions_it_went_in_at() {
        let mut pool: BufferPool<Buffer2<f32>> = BufferPool::default();

        let buffer = pool.acquire(SMALL);
        assert_eq!(buffer.width(), SMALL.width);
        assert_eq!(buffer.height(), SMALL.height);
        assert_eq!(pooled_count(&pool), 0, "acquire empties the free list");

        pool.release(buffer, SMALL);
        assert_eq!(pooled_count(&pool), 1);

        let reused = pool.acquire(SMALL);
        assert_eq!(reused.width(), SMALL.width);
        assert_eq!(pooled_count(&pool), 0);
    }

    /// The size the pool was previously used at must not leak through to a caller asking for a
    /// different one — downstream SIMD kernels index off the dimensions they asked for.
    #[test]
    #[should_panic(expected = "assertion `left == right` failed")]
    fn acquiring_at_new_dimensions_without_clearing_panics() {
        let mut pool: BufferPool<Buffer2<f32>> = BufferPool::default();

        let small = pool.acquire(SMALL);
        pool.release(small, SMALL);

        let _stale = pool.acquire(LARGE);
    }

    /// `clear` is what makes a size change legal, and is what `DetectionResources::reset` calls.
    #[test]
    fn clearing_between_sizes_allocates_afresh() {
        let mut pool: BufferPool<Buffer2<f32>> = BufferPool::default();

        let small = pool.acquire(SMALL);
        pool.release(small, SMALL);
        pool.clear();
        assert_eq!(pooled_count(&pool), 0);

        let large = pool.acquire(LARGE);
        assert_eq!(large.width(), LARGE.width);
        assert_eq!(large.height(), LARGE.height);
    }
}
