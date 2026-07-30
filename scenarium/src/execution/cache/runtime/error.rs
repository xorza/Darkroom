//! What an eviction sweep reports back.

use crate::execution::identity::ExecutionNodeId;

/// One node the sweep could not evict, named by the id the host asked in.
///
/// Not a `Result`: a sweep covers many nodes and a failure on one says nothing
/// about the rest, so the whole sweep runs and hands back what it could not do.
#[derive(Debug)]
pub(crate) struct CacheEvictionFailure {
    pub(crate) e_node_id: ExecutionNodeId,
    pub(crate) message: String,
}
