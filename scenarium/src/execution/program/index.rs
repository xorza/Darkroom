use std::ops::{Index, IndexMut};

use common::Span;
use hashbrown::{HashMap, HashSet};

use crate::execution::identity::ExecutionNodeId;

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
/// their compiled output span, while individual entries require an [`OutputIdx`].
#[derive(Debug, Clone, Default)]
pub(crate) struct OutputColumn<T> {
    pub(crate) values: Vec<T>,
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

pub(crate) type NodeMap<T> = HashMap<ExecutionNodeId, T>;
pub(crate) type NodeSet = HashSet<ExecutionNodeId>;

impl<T> OutputColumn<T> {
    pub(crate) fn slice(&self, outputs: Span) -> &[T] {
        &self.values[outputs.range()]
    }

    pub(crate) fn slice_mut(&mut self, outputs: Span) -> &mut [T] {
        &mut self.values[outputs.range()]
    }
}
