//! A dense column and the index space it is keyed by.
//!
//! Two things share the shape: the installed program's per-node and per-output
//! state. Both want array reads with no id hashing, and neither may be read
//! with the other's index — which is what the [`Idx`] parameter enforces.

use std::marker::PhantomData;
use std::ops::{Index, IndexMut, Range};

/// A scalar position in one dense space.
///
/// What makes [`Column`] safe to share between spaces: the column carries the
/// index type it is aligned to, so one space's index cannot reach into
/// another's column even when both are `u32` underneath.
pub(crate) trait Idx: Copy {
    fn idx(self) -> usize;

    /// The index at flat position `i` — how a walk over a column's entries
    /// names the slot it is at.
    fn from_idx(i: usize) -> Self;
}

/// A dense column of `T` aligned to the space `I` indexes — the per-run state
/// shape: resets are memsets and lookups are array reads, with no id hashing
/// anywhere.
///
/// `I` is carried, not stored: it is what refuses an index from the *other*
/// space at compile time, so a column keyed by one `…Idx` cannot be read with
/// another.
#[derive(Debug, Clone)]
pub(crate) struct Column<I, T> {
    values: Vec<T>,
    space: PhantomData<I>,
}

/// An empty column, for any `I` and `T`. Derived, this would demand
/// `T: Default` — a bound the empty vector does not need, and one a column
/// filled by [`push`](Self::push) rather than [`reset`](Self::reset) has no
/// value to satisfy it with.
impl<I, T> Default for Column<I, T> {
    fn default() -> Self {
        Self {
            values: Vec::new(),
            space: PhantomData,
        }
    }
}

impl<I, T> From<Vec<T>> for Column<I, T> {
    fn from(values: Vec<T>) -> Self {
        Self {
            values,
            space: PhantomData,
        }
    }
}

impl<I, T: Clone> Column<I, T> {
    pub(crate) fn reset(&mut self, len: usize, value: T) {
        self.values.clear();
        self.values.resize(len, value);
    }
}

impl<I, T> Column<I, T> {
    /// The index space the column spans — what a validator checks before
    /// reading it by index.
    pub(crate) fn len(&self) -> usize {
        self.values.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Append the entry for the next dense index — build-time only; per-run
    /// columns size themselves with [`reset`](Self::reset).
    pub(crate) fn push(&mut self, value: T) {
        self.values.push(value);
    }

    pub(crate) fn clear(&mut self) {
        self.values.clear();
    }

    pub(crate) fn iter(&self) -> std::slice::Iter<'_, T> {
        self.values.iter()
    }

    pub(crate) fn drain(&mut self) -> std::vec::Drain<'_, T> {
        self.values.drain(..)
    }
}

impl<I: Idx, T> Column<I, T> {
    /// The entry at `index`, or `None` past the column's length — for callers
    /// probing a column that may not span the program (an empty pre-run
    /// column, a validation bounds check). In-range access uses indexing.
    pub(crate) fn get(&self, index: I) -> Option<&T> {
        self.values.get(index.idx())
    }

    /// Entries paired with the index they sit at, so a walk that needs both
    /// doesn't rebuild the index from an enumeration counter.
    pub(crate) fn iter_indexed(&self) -> impl Iterator<Item = (I, &T)> {
        self.values
            .iter()
            .enumerate()
            .map(|(i, value)| (I::from_idx(i), value))
    }
}

impl<I: Idx, T> Index<I> for Column<I, T> {
    type Output = T;

    fn index(&self, index: I) -> &T {
        &self.values[index.idx()]
    }
}

impl<I: Idx, T> IndexMut<I> for Column<I, T> {
    fn index_mut(&mut self, index: I) -> &mut T {
        &mut self.values[index.idx()]
    }
}

impl<I, T> Column<I, T> {
    /// A contiguous run of the column. The caller resolves the run from
    /// whatever names it — a node's compiled output range, say — so the
    /// column itself stays free of any one space's vocabulary.
    pub(crate) fn slice(&self, range: Range<usize>) -> &[T] {
        &self.values[range]
    }

    pub(crate) fn slice_mut(&mut self, range: Range<usize>) -> &mut [T] {
        &mut self.values[range]
    }
}

#[cfg(test)]
mod internals {
    use crate::common::column::Column;

    impl<I, T> Column<I, T> {
        /// The backing allocation, for tests asserting a per-run column is refilled
        /// rather than reallocated.
        pub(crate) fn capacity(&self) -> usize {
            self.values.capacity()
        }
    }
}

#[cfg(test)]
mod tests;
