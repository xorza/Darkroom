//! What an eviction sweep reports back.

use crate::graph::identity::NodeId;

/// One node the sweep could not evict, named by the id the host asked in.
///
/// Not a `Result`: a sweep covers many nodes and a failure on one says nothing
/// about the rest, so the whole sweep runs and hands back what it could not do.
#[derive(Debug)]
pub(crate) struct CacheEvictionFailure {
    pub(crate) node_id: NodeId,
    pub(crate) message: String,
}
