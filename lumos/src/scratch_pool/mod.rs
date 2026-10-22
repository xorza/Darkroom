//! [`ScratchPool`]: values reused across stages and jobs, each back in the pool when its lease
//! drops.
//!
//! A pool is fixed to one [`Scratch::Shape`], and builds and refits its values at it. A lease
//! ([`Pooled`]) owns its value and its share of the pool, so a stage can keep several in a struct
//! across later stages, and rayon workers can lease from one pool concurrently through `&self`.
//!
//! A rayon `for_each_init` init closure is the right home for scratch when the call *is* the
//! operation, as the X-Trans demosaic's tiles are: rayon drops what the init made when the call
//! ends. A pool is for a loop that runs many times over — once per chunk per channel in the
//! combine, once per pass in the cosmic-ray detector — where the init becomes `|| pool.acquire()`
//! and the next call finds its scratch warm.

use std::fmt::Debug;
use std::mem;
use std::mem::ManuallyDrop;
use std::ops::{Deref, DerefMut};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;

use imaginarium::Buffer2;

use crate::math::size2us::Size2us;

/// A value a [`ScratchPool`] builds at its shape and fits to that shape again on reuse.
pub(crate) trait Scratch: Send + Sized {
    type Shape: Copy + Eq + Debug + Send;

    /// A fresh value at `shape`.
    fn build(shape: Self::Shape) -> Self;

    /// Make a value that was last used at `shape` ready for reuse at it. Contents stay
    /// unspecified.
    fn fit(&mut self, shape: Self::Shape);

    /// Whether a returned value still has `shape`; a holder that swapped in a value of another
    /// shape broke the lease.
    fn has_shape(&self, shape: Self::Shape) -> bool;
}

/// Scratch with no shape of its own: working vectors a job sizes as it goes, built by `Default`
/// and reused as they are.
pub(crate) trait Shapeless: Default + Send {}

impl<T: Shapeless> Scratch for T {
    type Shape = ();

    fn build((): ()) -> Self {
        T::default()
    }

    fn fit(&mut self, (): ()) {}

    fn has_shape(&self, (): ()) -> bool {
        true
    }
}

/// An image plane: a value's shape is its dimensions, and it is reused as it is.
impl<T: Default + Clone + Send> Scratch for Buffer2<T> {
    type Shape = Size2us;

    fn build(shape: Size2us) -> Self {
        Self::new_default(shape.width, shape.height)
    }

    fn fit(&mut self, shape: Size2us) {
        debug_assert!(self.has_shape(shape));
    }

    fn has_shape(&self, shape: Size2us) -> bool {
        Size2us::from(self) == shape
    }
}

/// Values of one [`Scratch`] type at one shape, leased out and returned on drop.
///
/// A leased value's contents are **unspecified**: a fresh one is as [`Scratch::build`] makes it, a
/// reused one keeps what its last holder left. Overwrite before reading.
#[derive(Debug)]
pub(crate) struct ScratchPool<T: Scratch> {
    shelf: Arc<Mutex<Shelf<T>>>,
}

/// The shape a pool serves, and its free values.
#[derive(Debug)]
struct Shelf<T: Scratch> {
    shape: T::Shape,
    /// Bumped by every change of shape, so a value leased before one is dropped on return instead
    /// of shelved at a shape it does not have.
    generation: u64,
    free: Vec<T>,
}

impl<T: Scratch> Default for ScratchPool<T>
where
    T::Shape: Default,
{
    fn default() -> Self {
        Self::new(T::Shape::default())
    }
}

impl<T: Scratch> ScratchPool<T> {
    /// An empty pool serving `shape`. Nothing is built until the first [`Self::acquire`].
    pub(crate) fn new(shape: T::Shape) -> Self {
        Self {
            shelf: Arc::new(Mutex::new(Shelf {
                shape,
                generation: 0,
                free: Vec::new(),
            })),
        }
    }

    pub(crate) fn shape(&self) -> T::Shape {
        lock(&self.shelf).shape
    }

    /// Serve `shape` from now on. A change drops every free value, and every value still leased is
    /// dropped when it returns.
    #[expect(
        clippy::needless_pass_by_ref_mut,
        reason = "only the pool's owner reshapes it, never a lease"
    )]
    pub(crate) fn reshape(&mut self, shape: T::Shape) {
        let stale = {
            let mut shelf = lock(&self.shelf);
            if shelf.shape == shape {
                return;
            }
            shelf.shape = shape;
            shelf.generation += 1;
            mem::take(&mut shelf.free)
        };
        drop(stale);
    }

    /// Drop every free value. Values still leased return as usual.
    #[expect(
        clippy::needless_pass_by_ref_mut,
        reason = "only the pool's owner clears it, never a lease"
    )]
    pub(crate) fn clear(&mut self) {
        let free = mem::take(&mut lock(&self.shelf).free);
        drop(free);
    }

    /// A free value fitted to the pool's shape, or a fresh one.
    pub(crate) fn acquire(&self) -> Pooled<T> {
        let (reused, shape, generation) = {
            let mut shelf = lock(&self.shelf);
            (shelf.free.pop(), shelf.shape, shelf.generation)
        };
        let value = match reused {
            Some(mut value) => {
                value.fit(shape);
                value
            }
            None => T::build(shape),
        };
        Pooled {
            value: ManuallyDrop::new(value),
            shape,
            generation,
            shelf: Arc::clone(&self.shelf),
        }
    }
}

/// A value on lease from a [`ScratchPool`], returned to it on drop.
#[derive(Debug)]
pub(crate) struct Pooled<T: Scratch> {
    /// Moved back to the pool by `drop`, which is why it is not dropped in place.
    value: ManuallyDrop<T>,
    shape: T::Shape,
    generation: u64,
    shelf: Arc<Mutex<Shelf<T>>>,
}

impl<T: Scratch> Deref for Pooled<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.value
    }
}

impl<T: Scratch> DerefMut for Pooled<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.value
    }
}

impl<T: Scratch> Drop for Pooled<T> {
    /// A value of another shape is never shelved: it would be handed out as the pool's, and
    /// indexed as an image it is not. Release assert on it, once per lease, except while unwinding,
    /// where a second panic would abort.
    fn drop(&mut self) {
        let value = unsafe { ManuallyDrop::take(&mut self.value) };
        if !value.has_shape(self.shape) {
            assert!(
                thread::panicking(),
                "pooled value returned at a shape other than the one it was leased at"
            );
            return;
        }
        let mut shelf = lock(&self.shelf);
        if shelf.generation == self.generation {
            shelf.free.push(value);
        }
    }
}

/// The shelf, poisoned or not: every critical section on it is a push, a pop or a swap of plain
/// fields, so no panic can leave it half-updated.
fn lock<T: Scratch>(shelf: &Mutex<Shelf<T>>) -> MutexGuard<'_, Shelf<T>> {
    shelf.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(feature = "internals")]
pub(crate) mod internals {
    use std::mem::ManuallyDrop;

    #[cfg(test)]
    use crate::scratch_pool;
    use crate::scratch_pool::{Pooled, Scratch, ScratchPool};

    impl<T: Scratch> Pooled<T> {
        /// `value` on lease from a pool of its own, for a fixture built outside any pool.
        pub(crate) fn loose(value: T, shape: T::Shape) -> Self {
            debug_assert!(
                value.has_shape(shape),
                "a loose value has the shape it is leased at"
            );
            Self {
                value: ManuallyDrop::new(value),
                shape,
                generation: 0,
                shelf: ScratchPool::<T>::new(shape).shelf,
            }
        }
    }

    impl<T: Scratch> ScratchPool<T> {
        /// How many values are free: once warm, the high-water mark of concurrent leases.
        #[cfg(test)]
        pub(crate) fn pooled(&self) -> usize {
            scratch_pool::lock(&self.shelf).free.len()
        }

        /// Whether every free value passes `predicate`.
        #[cfg(test)]
        pub(crate) fn all_free(&self, predicate: impl Fn(&T) -> bool) -> bool {
            scratch_pool::lock(&self.shelf).free.iter().all(predicate)
        }
    }
}

#[cfg(test)]
mod tests;
