//! The installed program's **dense index space**: scalar positions (`…Idx`),
//! composite addresses (`…Addr`), and the columns and sets aligned to them.
//! All install-local — indices shift between compiles and must never enter a
//! digest, a persisted byte, or a host-facing report. Deliberately bare names:
//! these types never leave the execution internals, unlike the stable
//! `Execution`-prefixed identity space in `execution/identity.rs`.

use std::ops::{Index, IndexMut};

use crate::execution::program::OutputRange;

/// A position in the program's flat output pool. It cannot be confused with a node
/// id or a node-local port number.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct OutputIdx(pub(crate) u32);

impl OutputIdx {
    pub(crate) fn idx(self) -> usize {
        self.0 as usize
    }
}

impl From<usize> for OutputIdx {
    fn from(i: usize) -> Self {
        debug_assert!(
            u32::try_from(i).is_ok(),
            "output pool index must fit in u32"
        );
        OutputIdx(i as u32)
    }
}

/// A column aligned to the program's flat output pool. Node-local views are sliced by
/// their compiled output range, while individual entries require an [`OutputIdx`].
#[derive(Debug, Clone, Default)]
pub(crate) struct OutputColumn<T> {
    values: Vec<T>,
}

impl<T: Clone> OutputColumn<T> {
    pub(crate) fn reset(&mut self, len: usize, value: T) {
        self.values.clear();
        self.values.resize(len, value);
    }
}

impl<T> From<Vec<T>> for OutputColumn<T> {
    fn from(values: Vec<T>) -> Self {
        Self { values }
    }
}

impl<T> Index<OutputIdx> for OutputColumn<T> {
    type Output = T;

    fn index(&self, index: OutputIdx) -> &T {
        &self.values[index.idx()]
    }
}

impl<T> IndexMut<OutputIdx> for OutputColumn<T> {
    fn index_mut(&mut self, index: OutputIdx) -> &mut T {
        &mut self.values[index.idx()]
    }
}

/// A node's position in the installed program's dense node vector. Install-local:
/// indices shift between compiles, so a `NodeIdx` must never enter a digest, a
/// persisted byte, or any host-facing report — those stay on `ExecutionNodeId`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct NodeIdx(pub(crate) u32);

impl NodeIdx {
    pub(crate) fn idx(self) -> usize {
        self.0 as usize
    }
}

/// An [`ExecutionOutputPort`](crate::execution::identity::ExecutionOutputPort)
/// interned into the installed program's dense index space — the hash-free form
/// every per-run edge walk uses, resolved once at compile
/// ([`intern_bindings`](crate::execution::program::ExecutionProgram::intern_bindings)).
/// Install-local like [`NodeIdx`]: it must never enter a digest, a persisted
/// byte, or a host-facing report.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct OutputAddr {
    pub(crate) node_idx: NodeIdx,
    pub(crate) port_idx: u32,
}

/// A column aligned to the program's dense node vector — the per-run state shape:
/// resets are memsets and lookups are array reads, with no id hashing anywhere.
#[derive(Debug, Clone, Default)]
pub(crate) struct NodeColumn<T> {
    values: Vec<T>,
}

impl<T: Clone> NodeColumn<T> {
    pub(crate) fn reset(&mut self, len: usize, value: T) {
        self.values.clear();
        self.values.resize(len, value);
    }
}

impl<T> NodeColumn<T> {
    pub(crate) fn len(&self) -> usize {
        self.values.len()
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

impl<T> Index<NodeIdx> for NodeColumn<T> {
    type Output = T;

    fn index(&self, index: NodeIdx) -> &T {
        &self.values[index.idx()]
    }
}

impl<T> IndexMut<NodeIdx> for NodeColumn<T> {
    fn index_mut(&mut self, index: NodeIdx) -> &mut T {
        &mut self.values[index.idx()]
    }
}

/// A dense node-index bitset aligned to the installed program, for the plan's
/// root and seed sets. `len` is kept because word-granular indexing would let an
/// out-of-range index land in the last word's padding bits instead of panicking.
#[derive(Debug, Default)]
pub(crate) struct NodeSet {
    words: Vec<u64>,
    len: usize,
}

impl NodeSet {
    /// The index space the set spans — the bound every [`contains`](Self::contains)
    /// and [`insert`](Self::insert) is checked against.
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    pub(crate) fn reset(&mut self, len: usize) {
        self.words.clear();
        self.words.resize(len.div_ceil(u64::BITS as usize), 0);
        self.len = len;
    }

    pub(crate) fn insert(&mut self, node_idx: NodeIdx) {
        debug_assert!(node_idx.idx() < self.len, "node index out of range");
        self.words[node_idx.idx() / u64::BITS as usize] |=
            1 << (node_idx.idx() % u64::BITS as usize);
    }

    pub(crate) fn contains(&self, node_idx: NodeIdx) -> bool {
        debug_assert!(node_idx.idx() < self.len, "node index out of range");
        self.words[node_idx.idx() / u64::BITS as usize] >> (node_idx.idx() % u64::BITS as usize) & 1
            != 0
    }

    /// Ascending set indices. Empty words are skipped whole, so a one-node
    /// preview seed costs a scan of `len / 64` words rather than `len` bits.
    pub(crate) fn iter(&self) -> impl Iterator<Item = NodeIdx> + '_ {
        self.words
            .iter()
            .enumerate()
            .filter(|&(_, &word)| word != 0)
            .flat_map(|(w, &word)| {
                (0..u64::BITS)
                    .filter(move |bit| word >> bit & 1 != 0)
                    .map(move |bit| NodeIdx(w as u32 * u64::BITS + bit))
            })
    }
}

impl<T> OutputColumn<T> {
    pub(crate) fn slice(&self, outputs: OutputRange) -> &[T] {
        &self.values[outputs.range()]
    }

    pub(crate) fn slice_mut(&mut self, outputs: OutputRange) -> &mut [T] {
        &mut self.values[outputs.range()]
    }
}

#[cfg(test)]
mod test_support {
    use crate::execution::program::index::{NodeColumn, NodeIdx, OutputColumn};

    impl<T> OutputColumn<T> {
        pub(crate) fn iter(&self) -> std::slice::Iter<'_, T> {
            self.values.iter()
        }
    }

    impl<T> NodeColumn<T> {
        /// The entry at `node_idx`, or `None` past the column's length — for
        /// introspection that may run before the column is sized (an empty
        /// pre-run column). Production access is by indexing.
        pub(crate) fn get(&self, node_idx: NodeIdx) -> Option<&T> {
            self.values.get(node_idx.idx())
        }
    }
}

#[cfg(test)]
mod tests;
