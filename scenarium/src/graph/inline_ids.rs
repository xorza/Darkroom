//! [`InlineIds`]: a short list of node ids held inline, the body of an
//! [`InstancePath`](crate::InstancePath) and a [`ProgramId`](crate::ProgramId).

use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};

use crate::graph::identity::NodeId;

/// At most `N` node ids, held inline so the value carries no allocation. Compared, ordered and
/// hashed by the ids it holds, element by element: a list before every longer list it begins.
/// Refused: `arrayvec`, a dependency for this one use, whose `u32` length makes a
/// [`ProgramId`](crate::ProgramId) 132 bytes for its 129.
#[derive(Clone, Copy)]
pub(crate) struct InlineIds<const N: usize> {
    len: u8,
    /// The list in `..len`; the tail is nil, so no two equal lists differ in memory.
    ids: [NodeId; N],
}

impl<const N: usize> InlineIds<N> {
    /// The empty list.
    pub(crate) const EMPTY: Self = Self {
        len: 0,
        ids: [NodeId::nil(); N],
    };

    /// The list `ids`.
    ///
    /// # Panics
    /// If `ids` holds more than `N`, which each holder's own check refuses first.
    pub(crate) fn new(ids: &[NodeId]) -> Self {
        assert!(ids.len() <= N, "an inline list holds at most {N} ids");
        let mut list = Self::EMPTY;
        list.len = u8::try_from(ids.len()).expect("an inline list's length fits u8");
        list.ids[..ids.len()].copy_from_slice(ids);
        list
    }

    /// The ids, in order.
    pub(crate) fn ids(&self) -> &[NodeId] {
        &self.ids[..usize::from(self.len)]
    }

    /// This list with `id` after its ids.
    ///
    /// # Panics
    /// If it holds `N` ids already.
    #[must_use]
    pub(crate) fn pushed(mut self, id: NodeId) -> Self {
        let len = usize::from(self.len);
        assert!(len < N, "an inline list holds at most {N} ids");
        self.ids[len] = id;
        self.len += 1;
        self
    }
}

impl<const N: usize> PartialEq for InlineIds<N> {
    fn eq(&self, other: &Self) -> bool {
        self.ids() == other.ids()
    }
}

impl<const N: usize> Eq for InlineIds<N> {}

impl<const N: usize> PartialOrd for InlineIds<N> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<const N: usize> Ord for InlineIds<N> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.ids().cmp(other.ids())
    }
}

impl<const N: usize> Hash for InlineIds<N> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.ids().hash(state);
    }
}

impl<const N: usize> fmt::Debug for InlineIds<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.ids()).finish()
    }
}
