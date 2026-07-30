//! Strongly typed identities for one flattened compiled graph.
//!
//! Minting these is flattening's business, in [`crate::execution::flatten`].
//!
//! Naming convention: `Execution`-prefixed types are the **stable identity
//! space** — they survive installs, cross the host boundary, and may enter
//! digests. `…Id` is a uuid identity; `…Port` pairs one with a port/event
//! index. The install-local **dense index space** below (`NodeIdx`,
//! `OutputIdx`, `OutputAddr`, the [`IdxSet<NodeIdx>`] over them, and the
//! `…Idx` positions in the packed port columns) keeps bare names:
//! those types never leave the execution internals, indices shift between
//! compiles, and none of them may enter a digest, a persisted byte, or a
//! host-facing report.

use serde::{Deserialize, Serialize};

use crate::common::column::Idx;
use crate::graph::identity::NodeId;

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[repr(transparent)]
/// One node in a flattened compiled graph.
pub struct ExecutionNodeId(NodeId);

impl ExecutionNodeId {
    /// The execution identity of one authored node.
    ///
    /// A graph is flat, so this is the node's own id: every authored node
    /// becomes exactly one execution node, and the two spaces differ only in
    /// type. The distinction survives because they still mean different
    /// things — an `ExecutionNodeId` names a node in an installed program,
    /// which a document's id outlives — but nothing is derived any more.
    pub(crate) fn from_node(node_id: NodeId) -> Self {
        Self(node_id)
    }

    /// The authored node this identity names. The inverse of
    /// [`from_node`](Self::from_node), and the whole of attribution.
    pub(crate) fn node_id(self) -> NodeId {
        self.0
    }

    pub(crate) fn as_uuid(self) -> uuid::Uuid {
        self.0.as_uuid()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
/// One output port of one flattened execution node.
pub(crate) struct ExecutionOutputPort {
    pub(crate) e_node_id: ExecutionNodeId,
    pub(crate) port_idx: usize,
}

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
/// One event port of one flattened execution node.
pub struct ExecutionEventPort {
    pub e_node_id: ExecutionNodeId,
    pub event_idx: usize,
}

/// A node's position in the installed program's dense node vector. Install-local:
/// indices shift between compiles, so a `NodeIdx` must never enter a digest, a
/// persisted byte, or any host-facing report — those stay on `ExecutionNodeId`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct NodeIdx(pub(crate) u32);

impl Idx for NodeIdx {
    fn idx(self) -> usize {
        self.0 as usize
    }

    fn from_idx(i: usize) -> Self {
        debug_assert!(u32::try_from(i).is_ok(), "node index must fit in u32");
        NodeIdx(i as u32)
    }
}

/// An [`ExecutionOutputPort`](crate::execution::identity::ExecutionOutputPort)
/// interned into the installed program's dense index space — the hash-free form
/// every per-run edge walk uses, resolved once by the compile link stage.
/// Install-local like [`NodeIdx`]: it must never enter a digest, a persisted
/// byte, or a host-facing report.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct OutputAddr {
    pub(crate) node_idx: NodeIdx,
    pub(crate) port_idx: u32,
}

/// A position in the program's flat output pool. It cannot be confused with a node
/// id or a node-local port number.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct OutputIdx(pub(crate) u32);

impl Idx for OutputIdx {
    fn idx(self) -> usize {
        self.0 as usize
    }

    fn from_idx(i: usize) -> Self {
        debug_assert!(
            u32::try_from(i).is_ok(),
            "output pool index must fit in u32"
        );
        OutputIdx(i as u32)
    }
}

/// A position in the program's flat input column, packed one node's ports after
/// another. A node owns a [`Span<InputIdx>`](crate::common::column::Span); this
/// names one port inside it.
///
/// The space spans both compile stages: linking rebuilds flatten's input column
/// into the program's slot for slot, so a position means the same port in
/// either.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct InputIdx(pub(crate) u32);

impl Idx for InputIdx {
    fn idx(self) -> usize {
        self.0 as usize
    }

    fn from_idx(i: usize) -> Self {
        debug_assert!(
            u32::try_from(i).is_ok(),
            "input column index must fit in u32"
        );
        InputIdx(i as u32)
    }
}

/// A position in the program's flat event column — [`InputIdx`]'s counterpart
/// for event ports.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct EventIdx(pub(crate) u32);

impl Idx for EventIdx {
    fn idx(self) -> usize {
        self.0 as usize
    }

    fn from_idx(i: usize) -> Self {
        debug_assert!(
            u32::try_from(i).is_ok(),
            "event column index must fit in u32"
        );
        EventIdx(i as u32)
    }
}

#[cfg(any(test, feature = "internals"))]
pub(crate) mod internals {
    use crate::execution::identity::ExecutionNodeId;
    use crate::graph::identity::NodeId;

    impl ExecutionNodeId {
        pub fn unique() -> Self {
            Self(NodeId::unique())
        }

        pub const fn from_u128(value: u128) -> Self {
            Self(NodeId::from_u128(value))
        }
    }
}
