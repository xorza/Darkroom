//! Strongly typed identities for one flattened compiled graph.
//!
//! The attribution taking these ids back to authored nodes is flattening's own
//! record, so it lives with the walk that builds it in
//! [`crate::execution::flatten::attribution`].
//!
//! Naming convention: `Execution`-prefixed types are the **stable identity
//! space** — they survive installs, cross the host boundary, and may enter
//! digests. `…Id` is a uuid identity; `…Port` pairs one with a port/event
//! index. The install-local **dense index space** (`NodeIdx`, `OutputIdx`,
//! `OutputAddr`) lives in `program/index.rs` under bare names — those types
//! never leave the execution internals, so they need no prefix.

use serde::{Deserialize, Serialize};

use crate::graph::address::NodeId;

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
    /// [`CompiledGraph::run_targets`]: crate::execution::compile::CompiledGraph::run_targets
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

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
/// A failed lookup from an execution identity to its authoring attribution.
pub enum ExecutionIdentityError {
    #[error("execution node {e_node_id:?} has no authoring attribution in this compiled graph")]
    NodeNotFound { e_node_id: ExecutionNodeId },
}

#[cfg(any(test, feature = "internals"))]
pub(crate) mod internals {
    use crate::execution::identity::ExecutionNodeId;
    use crate::graph::address::NodeId;

    impl ExecutionNodeId {
        pub fn unique() -> Self {
            Self(NodeId::unique())
        }

        pub const fn from_u128(value: u128) -> Self {
            Self(NodeId::from_u128(value))
        }
    }
}
