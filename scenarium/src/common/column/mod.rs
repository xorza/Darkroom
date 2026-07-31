//! A dense column and the index space it is keyed by.
//!
//! Everything the execution side keeps per index shares this shape: per-node
//! and per-output run state, the program's port declarations, the artifact's
//! packed node lists. All of them want array reads with no id hashing, and none
//! may be read with another space's index — which is what the [`Idx`] parameter
//! enforces.
//!
//! A column is addressed two ways over the same space: by one [`Idx`], and by a
//! [`Span`] an owner holds when its entries are a packed run rather than one
//! slot. The second is what keeps a per-node input list out of a `Vec` per node
//! without inventing a second container.

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

/// A contiguous run of one dense space — one owner's slice of a [`Column`],
/// held by the owner while the entries stay packed in the column.
///
/// The span carries the *space*, not the element type, which is what lets one
/// span address every column over that space: a node's output run names its
/// slots in the type column and in each per-run column aligned to it, with
/// nothing to convert between them.
#[derive(Debug)]
pub(crate) struct Span<I> {
    pub(crate) start: u32,
    pub(crate) len: u32,
    space: PhantomData<I>,
}

impl<I> Span<I> {
    /// The run of `len` entries at `start` — how [`Column::append`] names what
    /// it just wrote, and how a stage that knows a run's size before its
    /// contents reserves one for a later stage to fill.
    pub(crate) fn new(start: u32, len: u32) -> Self {
        Self {
            start,
            len,
            space: PhantomData,
        }
    }

    /// The run's bounds in the column's flat space, for a caller slicing
    /// something else aligned to that space.
    pub(crate) fn range(self) -> Range<usize> {
        let start = self.start as usize;
        start..start + self.len as usize
    }
}

impl<I: Idx> Span<I> {
    /// The index `offset` entries into the run — how a caller holding an
    /// owner-local port number reaches the flat position it names.
    pub(crate) fn nth(self, offset: u32) -> I {
        debug_assert!(offset < self.len, "span offset is out of range");
        debug_assert!(
            self.start.checked_add(offset).is_some(),
            "a column index must fit in u32"
        );
        I::from_idx(self.start.wrapping_add(offset) as usize)
    }
}

/// Hand-written, like the three below: deriving would demand the same of `I`,
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
    /// An empty column that will hold `capacity` entries without growing —
    /// for a builder that counted its entries before filling them, so the
    /// column allocates once instead of doubling its way to the same size.
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self {
            values: Vec::with_capacity(capacity),
            space: PhantomData,
        }
    }

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

    /// The run `span` names, or `None` when it reaches past the column — the
    /// checked form of indexing by a span, for a validator holding one it has
    /// not yet proven belongs to this column.
    pub(crate) fn get_span(&self, span: Span<I>) -> Option<&[T]> {
        self.values.get(span.range())
    }

    /// Pack `values` at the end and hand back the run they occupy — how an
    /// owner of a variable-length list gets one without a `Vec` of its own.
    /// Build-time only, like [`push`](Self::push).
    pub(crate) fn append(&mut self, values: impl IntoIterator<Item = T>) -> Span<I> {
        let start = u32::try_from(self.values.len()).expect("column span start exceeds u32");
        self.values.extend(values);
        let end = u32::try_from(self.values.len()).expect("column length exceeds u32");
        Span::new(start, end - start)
    }

    /// The index holding `value`, or `None` — a binary search, so the column
    /// must be sorted. Sortedness is the filler's invariant; nothing here
    /// maintains it.
    ///
    /// What lets a column that is *already* ordered by its entries serve as its
    /// own lookup, instead of a side map allocated per build over the same
    /// arrangement.
    pub(crate) fn search_sorted(&self, value: &T) -> Option<I>
    where
        T: Ord,
    {
        self.values.binary_search(value).ok().map(I::from_idx)
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

impl<I, T> Index<Span<I>> for Column<I, T> {
    type Output = [T];

    fn index(&self, span: Span<I>) -> &[T] {
        &self.values[span.range()]
    }
}

impl<I, T> IndexMut<Span<I>> for Column<I, T> {
    fn index_mut(&mut self, span: Span<I>) -> &mut [T] {
        &mut self.values[span.range()]
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
