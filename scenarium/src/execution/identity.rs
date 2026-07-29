//! Strongly typed identities for one flattened compiled graph.
//!
//! Where each of these ids came from is flattening's own record, kept with the
//! walk that builds it in [`crate::execution::compile::flatten`]; the form a
//! host queries is the artifact's, in
//! [`crate::execution::source_map`].
//!
//! Naming convention: `Execution`-prefixed types are the **stable identity
//! space** — they survive installs, cross the host boundary, and may enter
//! digests. `…Id` is a uuid identity; `…Port` pairs one with a port/event
//! index. The install-local **dense index space** below (`NodeIdx`,
//! `OutputIdx`, `OutputAddr`, and the [`IdxSet<NodeIdx>`] over them) keeps bare names:
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
    /// Derive an execution identity from a non-empty authoring path, ordered
    /// from the outermost graph instance to the leaf node. A root node uses
    /// `[node_id]`; a nested node uses `[outer_instance, ..., node_id]`.
    ///
    /// Minting ids is flatten's business, so this stays inside the crate.
    /// Deriving one answers only for a node flatten emits: a composite
    /// dissolves and has no id of its own, so a host asking "which execution
    /// nodes is this?" would get an id that exists nowhere. Hosts look the
    /// relation up instead — [`CompiledGraph::run_targets`] and its siblings
    /// (`attribution`, `is_sink`, `is_impure`), which answer for every
    /// authored node.
    ///
    /// [`CompiledGraph::run_targets`]: crate::execution::compiled::CompiledGraph::run_targets
    pub(crate) fn from_authoring(path: &[NodeId]) -> Self {
        let (&node_id, instances) = path
            .split_last()
            .expect("an authoring path must include its leaf node");
        if instances.is_empty() {
            return Self(node_id);
        }
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"scenarium.flatten.v1");
        for instance in instances {
            hasher.update(&instance.as_u128().to_le_bytes());
        }
        hasher.update(&node_id.as_u128().to_le_bytes());
        let digest = hasher.finalize();
        Self(NodeId::from_u128(u128::from_le_bytes(
            digest.as_bytes()[..16].try_into().unwrap(),
        )))
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
