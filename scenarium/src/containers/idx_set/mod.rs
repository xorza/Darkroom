//! A dense bitset over an index space — [`Column`](crate::containers::column::Column)'s
//! set-shaped sibling, keyed the same way and with the same guarantee that one
//! space's index cannot reach into another's set.

use std::marker::PhantomData;
use std::slice;

use crate::containers::column::Idx;

/// A dense bitset over the space `I` indexes — a visited mark for a walk over
/// it (the schedule validator's, the eviction cone's).
///
/// `I` is carried, not stored, for the same reason [`Column`](crate::containers::column::Column)
/// carries it: one space's index cannot probe another's set. `len` is kept
/// because word-granular indexing would let an out-of-range index land in the
/// last word's padding bits instead of panicking.
#[derive(Debug)]
pub(crate) struct IdxSet<I> {
    words: Vec<u64>,
    len: usize,
    space: PhantomData<I>,
}

/// An empty set, for any `I`. Derived, this would demand `I: Default`, which
/// an index the set never stores has no reason to satisfy.
impl<I> Default for IdxSet<I> {
    fn default() -> Self {
        Self {
            words: Vec::new(),
            len: 0,
            space: PhantomData,
        }
    }
}

impl<I> IdxSet<I> {
    pub(crate) fn reset(&mut self, len: usize) {
        self.words.clear();
        self.words.resize(len.div_ceil(u64::BITS as usize), 0);
        self.len = len;
    }
}

impl<I: Idx> IdxSet<I> {
    pub(crate) fn insert(&mut self, index: I) {
        debug_assert!(index.idx() < self.len, "index out of range");
        self.words[index.idx() / u64::BITS as usize] |= 1 << (index.idx() % u64::BITS as usize);
    }

    pub(crate) fn contains(&self, index: I) -> bool {
        debug_assert!(index.idx() < self.len, "index out of range");
        (self.words[index.idx() / u64::BITS as usize] >> (index.idx() % u64::BITS as usize)) & 1
            != 0
    }

    /// Ascending set indices.
    pub(crate) fn iter(&self) -> IdxSetIter<'_, I> {
        IdxSetIter {
            words: self.words.iter(),
            word: 0,
            base: 0,
            next_base: 0,
            space: PhantomData,
        }
    }
}

impl<'a, I: Idx> IntoIterator for &'a IdxSet<I> {
    type Item = I;
    type IntoIter = IdxSetIter<'a, I>;

    fn into_iter(self) -> IdxSetIter<'a, I> {
        self.iter()
    }
}

/// [`IdxSet::iter`]'s iterator. Empty words are skipped whole and a set word
/// yields one index per set bit, so a one-node preview seed costs a scan of
/// `len / 64` words rather than `len` bits.
#[derive(Debug)]
pub(crate) struct IdxSetIter<'a, I> {
    words: slice::Iter<'a, u64>,
    /// The bits of the current word not yet yielded.
    word: u64,
    /// The index of the current word's bit 0.
    base: usize,
    /// The index of the next word's bit 0.
    next_base: usize,
    space: PhantomData<I>,
}

impl<I: Idx> Iterator for IdxSetIter<'_, I> {
    type Item = I;

    fn next(&mut self) -> Option<I> {
        while self.word == 0 {
            self.word = *self.words.next()?;
            self.base = self.next_base;
            self.next_base += u64::BITS as usize;
        }
        let bit = self.word.trailing_zeros() as usize;
        self.word &= self.word - 1;
        Some(I::from_idx(self.base + bit))
    }
}

#[cfg(test)]
mod tests;
