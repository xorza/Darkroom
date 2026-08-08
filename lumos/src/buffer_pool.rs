//! Dimension-checked reuse of image-sized buffers.

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
    pub(crate) fn acquire(&mut self, dimensions: Size2us) -> B {
        self.free.pop().unwrap_or_else(|| B::allocate(dimensions))
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
