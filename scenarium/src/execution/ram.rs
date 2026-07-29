//! Per-node RAM accounting for a run.
//!
//! A leaf of the execution tree:
//! [`RuntimeCache::resident_ram_stats`](crate::execution::cache::runtime::RuntimeCache::resident_ram_stats)
//! measures these — the cache is what knows what is resident — and
//! [`ExecutionOutcome`](crate::execution::outcome::ExecutionOutcome) collects them.
//! Kept apart from both so the outcome does not depend on the cache to name a
//! measurement the cache merely produces.

use crate::RamUsage;
use crate::execution::identity::ExecutionNodeId;

/// One node's share of the resident RAM.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NodeRamUsage {
    pub(crate) e_node_id: ExecutionNodeId,
    pub(crate) usage: RamUsage,
}
