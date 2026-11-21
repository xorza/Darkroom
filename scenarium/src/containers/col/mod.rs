//! A dense column and the index space it is keyed by.
//!
//! Everything the execution side keeps per index (run state, port declarations, packed node
//! lists) wants array reads with no id hashing, and none may be read with another space's
//! index, which the [`Idx`] parameter enforces.
//!
//! A column is addressed two ways over the same space: by one [`Idx`], and by a [`Span`] an
//! owner holds when its entries are a packed run, which keeps a per-node input list out of a
//! `Vec` per node.
//!
//! # Decisions
//!
//! - **An index's field is private**, built through [`Idx::from_idx`] alone, as rustc's
//!   [`newtype_index`](https://doc.rust-lang.org/nightly/nightly-rustc/rustc_index/macro.newtype_index.html)
//!   and cranelift's [entity references](https://docs.rs/cranelift-entity/latest/cranelift_entity/)
//!   keep theirs behind accessors. Refused: a niche for `Option<NodeIdx>`, which no column
//!   needs.

use std::marker::PhantomData;
use std::ops::{Index, IndexMut, Range};
use std::slice::{Iter, IterMut};
use std::vec::Drain;

/// A scalar position in one dense space.
///
/// The column carries the index type it is aligned to, so one space's index cannot reach into
/// another's column even when both are `u32` underneath.
pub(crate) trait Idx: Copy {
    fn idx(self) -> usize;

    /// The index at flat position `i`.
    fn from_idx(i: usize) -> Self;
}

/// Declare one dense index space: a `u32` newtype and its [`Idx`] impl.
///
/// `$what` names the space in the bound check; doc comments pass through, so what a space
/// *means* lives at the declaration. The field is private: an index is built through
/// [`Idx::from_idx`] alone, the route that checks the bound.
macro_rules! idx_type {
    ($(#[$meta:meta])* $vis:vis struct $name:ident, $what:literal) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
        $vis struct $name(u32);

        impl $crate::containers::col::Idx for $name {
            fn idx(self) -> usize {
                self.0 as usize
            }

            fn from_idx(i: usize) -> Self {
                debug_assert!(
                    u32::try_from(i).is_ok(),
                    concat!($what, " must fit in u32"),
                );
                $name(i as u32)
            }
        }
    };
}
pub(crate) use idx_type;

/// A contiguous run of one dense space — one owner's slice of a [`Col`],
/// held by the owner while the entries stay packed in the column.
///
/// The span carries the *space*, not the element type, so one span addresses every column over
/// that space.
#[derive(Debug)]
pub(crate) struct Span<I> {
    start: u32,
    len: u32,
    space: PhantomData<I>,
}

impl<I> Span<I> {
    /// The run of `len` entries at `start`.
    pub(crate) const fn new(start: u32, len: u32) -> Self {
        Self {
            start,
            len,
            space: PhantomData,
        }
    }

    /// How many entries the run holds.
    pub(crate) const fn len(self) -> usize {
        self.len as usize
    }

    /// Each offset into the run, as [`Self::nth`] takes it: a node's port numbers.
    pub(crate) const fn offsets(self) -> Range<u32> {
        0..self.len
    }

    pub(crate) const fn is_empty(self) -> bool {
        self.len == 0
    }

    /// The run's bounds in the column's flat space.
    pub(crate) const fn range(self) -> Range<usize> {
        let start = self.start as usize;
        start..start + self.len as usize
    }
}

impl<I: Idx> Span<I> {
    /// The index `offset` entries into the run.
    pub(crate) fn nth(self, offset: u32) -> I {
        debug_assert!(offset < self.len, "span offset is out of range");
        debug_assert!(
            self.start.checked_add(offset).is_some(),
            "a column index must fit in u32"
        );
        I::from_idx(self.start.wrapping_add(offset) as usize)
    }
}

/// Hand-written, like the impls below: deriving would demand the same of `I`,
/// which is a space marker no span ever stores.
impl<I> Clone for Span<I> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<I> Copy for Span<I> {}

impl<I> Default for Span<I> {
    fn default() -> Self {
        Self::new(0, 0)
    }
}

impl<I> PartialEq for Span<I> {
    fn eq(&self, other: &Self) -> bool {
        self.start == other.start && self.len == other.len
    }
}

impl<I> Eq for Span<I> {}

/// A dense column of `T` aligned to the space `I` indexes: resets are memsets and lookups are
/// array reads. `I` is carried, not stored, so a column keyed by one `…Idx` cannot be read with
/// another.
#[derive(Debug)]
pub(crate) struct Col<I, T> {
    values: Vec<T>,
    space: PhantomData<I>,
}

/// Hand-written for `clone_from`, which the derive leaves at `*self = source.clone()`: a per-run
/// column copied from another keeps its own buffer, as [`Vec::clone_from`] does.
impl<I, T: Clone> Clone for Col<I, T> {
    fn clone(&self) -> Self {
        Self {
            values: self.values.clone(),
            space: PhantomData,
        }
    }

    fn clone_from(&mut self, source: &Self) {
        self.values.clone_from(&source.values);
    }
}

/// An empty column, for any `I` and `T`. Derived, this would demand
/// `T: Default` — a bound the empty vector does not need, and one a column
/// filled by [`push`](Self::push) rather than [`reset`](Self::reset) has no
/// value to satisfy it with.
impl<I, T> Default for Col<I, T> {
    fn default() -> Self {
        Self {
            values: Vec::new(),
            space: PhantomData,
        }
    }
}

impl<I, T> FromIterator<T> for Col<I, T> {
    fn from_iter<V: IntoIterator<Item = T>>(values: V) -> Self {
        Self {
            values: values.into_iter().collect(),
            space: PhantomData,
        }
    }
}

impl<I, T, const N: usize> From<[T; N]> for Col<I, T> {
    fn from(values: [T; N]) -> Self {
        values.into_iter().collect()
    }
}

impl<I, T> Extend<T> for Col<I, T> {
    fn extend<V: IntoIterator<Item = T>>(&mut self, values: V) {
        self.values.extend(values);
    }
}

impl<'a, I, T> IntoIterator for &'a Col<I, T> {
    type Item = &'a T;
    type IntoIter = Iter<'a, T>;

    fn into_iter(self) -> Iter<'a, T> {
        self.values.iter()
    }
}

impl<I, T: Clone> Col<I, T> {
    pub(crate) fn reset(&mut self, len: usize, value: T) {
        self.values.clear();
        self.values.resize(len, value);
    }
}

impl<I, T> Col<I, T> {
    /// [`reset`](Self::reset) for an entry that is not `Clone`: each of the `len` entries is a
    /// fresh `fill()`.
    pub(crate) fn reset_with(&mut self, len: usize, fill: impl FnMut() -> T) {
        self.values.clear();
        self.values.resize_with(len, fill);
    }

    /// An empty column that will hold `capacity` entries without growing.
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self {
            values: Vec::with_capacity(capacity),
            space: PhantomData,
        }
    }

    /// The index space the column spans.
    pub(crate) const fn len(&self) -> usize {
        self.values.len()
    }

    pub(crate) const fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Append the entry for the next dense index; build-time only, as per-run columns size
    /// themselves with [`reset`](Self::reset).
    pub(crate) fn push(&mut self, value: T) {
        self.values.push(value);
    }

    pub(crate) fn clear(&mut self) {
        self.values.clear();
    }

    pub(crate) fn iter(&self) -> Iter<'_, T> {
        self.values.iter()
    }

    pub(crate) fn iter_mut(&mut self) -> IterMut<'_, T> {
        self.values.iter_mut()
    }

    pub(crate) fn drain(&mut self) -> Drain<'_, T> {
        self.values.drain(..)
    }
}

impl<I: Idx, T> Col<I, T> {
    /// The entry at `idx`, or `None` past the column's length, for a column that may not span the
    /// program (an empty pre-run column, a validation bounds check).
    pub(crate) fn get(&self, idx: I) -> Option<&T> {
        self.values.get(idx.idx())
    }

    /// The run `span` names, or `None` when it reaches past the column.
    pub(crate) fn get_span(&self, span: Span<I>) -> Option<&[T]> {
        self.values.get(span.range())
    }

    /// Pack `values` at the end and hand back the run they occupy; build-time only, like
    /// [`push`](Self::push).
    pub(crate) fn append(&mut self, values: impl IntoIterator<Item = T>) -> Span<I> {
        let start = u32::try_from(self.values.len()).expect("column span start exceeds u32");
        self.values.extend(values);
        let end = u32::try_from(self.values.len()).expect("column length exceeds u32");
        Span::new(start, end - start)
    }

    /// The index holding `value`, or `None`: a binary search, so the column must be sorted, which
    /// is the filler's invariant. It lets an already-ordered column serve as its own lookup
    /// instead of a side map.
    pub(crate) fn search_sorted(&self, value: &T) -> Option<I>
    where
        T: Ord,
    {
        self.values.binary_search(value).ok().map(I::from_idx)
    }

    /// Whether each entry is above the one before: what [`Self::search_sorted`] needs, and each
    /// key once.
    pub(crate) fn is_strictly_ascending(&self) -> bool
    where
        T: Ord,
    {
        self.values.is_sorted_by(|before, after| before < after)
    }

    /// Entries paired with the index they sit at.
    pub(crate) fn iter_indexed(&self) -> impl Iterator<Item = (I, &T)> {
        self.values
            .iter()
            .enumerate()
            .map(|(i, value)| (I::from_idx(i), value))
    }
}

impl<I: Idx, T> Index<I> for Col<I, T> {
    type Output = T;

    fn index(&self, idx: I) -> &T {
        &self.values[idx.idx()]
    }
}

impl<I: Idx, T> IndexMut<I> for Col<I, T> {
    fn index_mut(&mut self, idx: I) -> &mut T {
        &mut self.values[idx.idx()]
    }
}

impl<I, T> Index<Span<I>> for Col<I, T> {
    type Output = [T];

    fn index(&self, span: Span<I>) -> &[T] {
        &self.values[span.range()]
    }
}

impl<I, T> IndexMut<Span<I>> for Col<I, T> {
    fn index_mut(&mut self, span: Span<I>) -> &mut [T] {
        &mut self.values[span.range()]
    }
}

#[cfg(test)]
mod internals {
    use crate::containers::col::Col;

    impl<I, T> Col<I, T> {
        /// The backing allocation, for tests asserting a per-run column is refilled
        /// rather than reallocated.
        pub(crate) fn capacity(&self) -> usize {
            self.values.capacity()
        }
    }
}

#[cfg(test)]
mod tests;
