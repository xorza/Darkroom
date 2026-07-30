//! How the installed artifact and the cache aligned to it can disagree.

use thiserror::Error;

use crate::execution::identity::ExecutionNodeId;

/// What [`ExecutionEngine::validate`](crate::execution::engine::ExecutionEngine)
/// rejects: a cache that does not span the installed program's nodes, or a slot
/// that does not describe the node it sits on.
#[derive(Debug, Error)]
pub(super) enum InstallValidationError {
    #[error("runtime cache spans {slots} nodes, not the compiled program's {expected}")]
    NodeCount { slots: usize, expected: usize },
    #[error("runtime cache output arity does not match node {e_node_id:?}")]
    OutputArity { e_node_id: ExecutionNodeId },
    #[error("runtime cache state owner does not match node {e_node_id:?}")]
    StateOwner { e_node_id: ExecutionNodeId },
}
