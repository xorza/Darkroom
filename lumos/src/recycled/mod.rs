//! [`Recycled`]: a `Vec`'s allocation kept from one use to the next, for elements that borrow
//! something else.

use std::mem;
use std::mem::ManuallyDrop;

/// A `Vec`'s allocation, empty between uses, for elements that borrow what lives through one use
/// alone: [`Self::take`] lends it as a `Vec<U>`, `T` but for a shorter lifetime, and [`Self::put`]
/// keeps it again, emptied. A loop over chunks allocates it once rather than once a chunk.
#[derive(Debug)]
pub(crate) struct Recycled<T> {
    vec: Vec<T>,
}

impl<T> Default for Recycled<T> {
    fn default() -> Self {
        Self { vec: Vec::new() }
    }
}

impl<T> Recycled<T> {
    /// The allocation, empty, for elements of `U`.
    pub(crate) fn take<U>(&mut self) -> Vec<U> {
        emptied(mem::take(&mut self.vec))
    }

    /// Keep `vec`'s allocation for the next [`Self::take`], its elements dropped.
    pub(crate) fn put<U>(&mut self, vec: Vec<U>) {
        self.vec = emptied(vec);
    }
}

/// `vec`'s allocation with no elements, as a `Vec<U>`.
///
/// At each use `T` and `U` are one type but for a lifetime; a compile error holds them to one size,
/// not zero, and one alignment. That is the layout the allocation was made with, so it is the
/// layout `U`'s `Vec` frees it with, and with no element in it nothing of `U` is read before it is
/// written. The elements are dropped first, and the old `Vec` is kept from freeing the allocation.
fn emptied<T, U>(vec: Vec<T>) -> Vec<U> {
    const {
        assert!(
            size_of::<T>() == size_of::<U>()
                && size_of::<T>() != 0
                && align_of::<T>() == align_of::<U>(),
            "a recycled Vec keeps its element layout",
        );
    }
    let mut vec = ManuallyDrop::new(vec);
    vec.clear();
    let (pointer, capacity) = (vec.as_mut_ptr(), vec.capacity());
    unsafe { Vec::from_raw_parts(pointer.cast::<U>(), 0, capacity) }
}

#[cfg(test)]
mod tests;
