//! [`ScratchPool`]: values reused across stages and jobs, each back in the pool when its lease drops.
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
use std::ptr;
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
        Buffer2::new_default(shape.width, shape.height)
    }

    fn fit(&mut self, shape: Size2us) {
        debug_assert!(self.has_shape(shape));
    }

    fn has_shape(&self, shape: Size2us) -> bool {
        Size2us::new(self.width(), self.height()) == shape
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

impl<T: Scratch> Pooled<T> {
    /// The value, leaving the pool for good: what a warp filled becomes the frame it parks.
    pub(crate) fn into_inner(self) -> T {
        let mut lease = ManuallyDrop::new(self);
        // SAFETY: `lease` is never dropped, so `value` is taken exactly once, here, and `shelf` is
        // read out of it exactly once, so its count is released by the drop below.
        let (value, shelf) = unsafe {
            (
                ManuallyDrop::take(&mut lease.value),
                ptr::read(&raw const lease.shelf),
            )
        };
        drop(shelf);
        value
    }
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
        // SAFETY: `value` is taken exactly once, here, and the lease is never read after its drop.
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

#[cfg(any(test, feature = "internals"))]
pub(crate) mod internals {
    use std::mem::ManuallyDrop;

    #[cfg(test)]
    use crate::scratch_pool;
    use crate::scratch_pool::{Pooled, Scratch, ScratchPool};

    impl<T: Scratch> Pooled<T> {
        /// `value` on lease from a pool of its own, for a fixture built outside any pool.
        pub(crate) fn loose(value: T, shape: T::Shape) -> Self {
            assert!(
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
mod tests {
    use std::panic;
    use std::panic::AssertUnwindSafe;
    use std::sync::Arc;

    use imaginarium::Buffer2;

    use crate::internals;
    use crate::math::size2us::Size2us;
    use crate::scratch_pool::{Scratch, ScratchPool, Shapeless};

    const SMALL: Size2us = Size2us {
        width: 8,
        height: 4,
    };
    const LARGE: Size2us = Size2us {
        width: 16,
        height: 4,
    };

    /// A vector whose shape is its length, built and fitted by resizing: what makes a reused value
    /// distinguishable from a fresh one is the fill it kept.
    #[derive(Debug)]
    struct Row(Vec<u8>);

    impl Scratch for Row {
        type Shape = usize;

        fn build(shape: usize) -> Self {
            Self(vec![0; shape])
        }

        fn fit(&mut self, shape: usize) {
            self.0.resize(shape, 0);
        }

        fn has_shape(&self, shape: usize) -> bool {
            self.0.len() == shape
        }
    }

    /// An acquire from an empty pool builds; a dropped lease is shelved and the next acquire hands
    /// back that same allocation, contents and all; two leases at once are two allocations.
    #[test]
    fn a_dropped_lease_is_the_next_acquire() {
        let pool: ScratchPool<Buffer2<f32>> = ScratchPool::new(SMALL);
        assert_eq!(pool.pooled(), 0, "nothing is built up front");

        let mut first = pool.acquire();
        assert_eq!((first.width(), first.height()), (8, 4));
        first.pixels_mut()[0] = 7.0;
        let address = first.as_ptr();
        drop(first);
        assert_eq!(pool.pooled(), 1);

        let again = pool.acquire();
        assert_eq!(again.as_ptr(), address);
        assert_eq!(
            again.pixels()[0],
            7.0,
            "a reused value keeps what its holder left"
        );
        let fresh = pool.acquire();
        assert_ne!(fresh.as_ptr(), address);
        assert_eq!(pool.pooled(), 0);
        drop((again, fresh));
        assert_eq!(pool.pooled(), 2);
    }

    /// A reused value is fitted to the pool's shape, a fresh one built at it: `Row`'s fit keeps
    /// the first `min(old, new)` bytes, so the kept fill tells the two apart.
    #[test]
    fn acquire_builds_and_fits_at_the_pool_shape() {
        let pool: ScratchPool<Row> = ScratchPool::new(3);
        let mut row = pool.acquire();
        assert_eq!(row.0, [0, 0, 0]);
        row.0.fill(9);
        drop(row);
        assert_eq!(pool.acquire().0, [9, 9, 9]);
    }

    /// A change of shape empties the shelf, and a lease taken before it is dropped on return
    /// rather than shelved at the new shape; the same shape again changes nothing.
    #[test]
    fn reshape_drops_free_and_outstanding_values() {
        let mut pool: ScratchPool<Buffer2<f32>> = ScratchPool::new(SMALL);
        drop(pool.acquire());
        let outstanding = pool.acquire();
        drop(pool.acquire());
        assert_eq!(pool.pooled(), 1);

        pool.reshape(SMALL);
        assert_eq!(pool.pooled(), 1, "the same shape keeps the shelf");

        pool.reshape(LARGE);
        assert_eq!(pool.shape(), LARGE);
        assert_eq!(pool.pooled(), 0);
        drop(outstanding);
        assert_eq!(pool.pooled(), 0, "a value of the old shape is not shelved");

        let large = pool.acquire();
        assert_eq!((large.width(), large.height()), (16, 4));
    }

    /// A holder that swapped in a value of another shape broke the lease: caught on return, and
    /// never shelved, also when the lease drops in an unwind that is already under way.
    #[test]
    fn a_value_of_another_shape_is_never_shelved() {
        let pool: ScratchPool<Buffer2<f32>> = ScratchPool::new(SMALL);
        let mut lease = pool.acquire();
        *lease = Buffer2::new_default(LARGE.width, LARGE.height);
        let payload = panic::catch_unwind(AssertUnwindSafe(|| drop(lease))).unwrap_err();
        assert_eq!(
            internals::panic_message(&*payload),
            "pooled value returned at a shape other than the one it was leased at"
        );
        assert_eq!(pool.pooled(), 0);

        let payload = panic::catch_unwind(AssertUnwindSafe(|| {
            let mut lease = pool.acquire();
            *lease = Buffer2::new_default(LARGE.width, LARGE.height);
            panic!("the holder's own failure");
        }))
        .unwrap_err();
        assert_eq!(
            internals::panic_message(&*payload),
            "the holder's own failure"
        );
        assert_eq!(pool.pooled(), 0);
    }

    /// A lease holds its share of the pool, so it outlives the pool handle it came from.
    #[test]
    fn a_lease_outlives_its_pool_handle() {
        let pool: ScratchPool<Row> = ScratchPool::default();
        let lease = pool.acquire();
        drop(pool);
        assert!(lease.0.is_empty());
    }

    /// `into_inner` takes a value out for good, releasing the lease's share of the pool; `clear`
    /// drops the free values, so the next acquire builds afresh.
    #[test]
    fn into_inner_and_clear_take_values_out_of_the_pool() {
        let mut pool: ScratchPool<Row> = ScratchPool::new(2);
        let mut kept = pool.acquire();
        kept.0.fill(5);
        let mut returned = pool.acquire();
        assert_eq!(Arc::strong_count(&pool.shelf), 3);

        let kept = kept.into_inner();
        assert_eq!(kept.0, [5, 5]);
        assert_eq!(Arc::strong_count(&pool.shelf), 2);
        drop(kept);
        assert_eq!(pool.pooled(), 0, "a value taken out is not shelved");

        returned.0.fill(6);
        drop(returned);
        assert_eq!(pool.pooled(), 1);
        pool.clear();
        assert_eq!(pool.pooled(), 0);
        assert_eq!(pool.acquire().0, [0, 0], "built afresh after the clear");
    }

    /// A `Shapeless` type pools at the unit shape: built by `Default`, reused as it was left.
    #[test]
    fn shapeless_scratch_is_reused_as_it_was_left() {
        #[derive(Debug, Default)]
        struct Notes(Vec<u8>);
        impl Shapeless for Notes {}

        let pool: ScratchPool<Notes> = ScratchPool::default();
        let mut notes = pool.acquire();
        assert!(notes.0.is_empty());
        notes.0.extend([1, 2, 3]);
        drop(notes);
        assert_eq!(pool.acquire().0, [1, 2, 3]);
    }
}
