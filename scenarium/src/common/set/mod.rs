//! A dense bitset over an index space — [`Column`](super::column::Column)'s
//! set-shaped sibling, keyed the same way and with the same guarantee that one
//! space's index cannot reach into another's set.

use std::marker::PhantomData;

use crate::common::column::Idx;

/// A dense bitset over the space `I` indexes — the plan's root and seed sets.
///
/// `I` is carried, not stored, for the same reason [`Column`](super::column::Column)
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
        self.words[index.idx() / u64::BITS as usize] >> (index.idx() % u64::BITS as usize) & 1 != 0
    }

    /// Ascending set indices. Empty words are skipped whole, so a one-node
    /// preview seed costs a scan of `len / 64` words rather than `len` bits.
    pub(crate) fn iter(&self) -> impl Iterator<Item = I> + '_ {
        self.words
            .iter()
            .enumerate()
            .filter(|&(_, &word)| word != 0)
            .flat_map(|(w, &word)| {
                (0..u64::BITS)
                    .filter(move |bit| word >> bit & 1 != 0)
                    .map(move |bit| I::from_idx(w * u64::BITS as usize + bit as usize))
            })
    }
}

#[cfg(test)]
mod tests;
