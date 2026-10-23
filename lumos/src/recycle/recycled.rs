//! [`Recycled`]: a `Vec`'s allocation kept from one use to the next.

use std::mem;

use crate::recycle;

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
        recycle::emptied(mem::take(&mut self.vec))
    }

    /// Keep `vec`'s allocation for the next [`Self::take`], its elements dropped.
    pub(crate) fn put<U>(&mut self, vec: Vec<U>) {
        self.vec = recycle::emptied(vec);
    }
}
