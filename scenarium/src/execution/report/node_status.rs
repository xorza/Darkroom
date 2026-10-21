//! [`NodeStatus`]: one node's whole result for one run, and its [`NodeExecutionStatus`].

use crate::data::ram_usage::RamUsage;
use crate::execution::error::RunError;
use crate::graph::identity::NodeId;

/// What one node did this run, at the host boundary — the published projection of the
/// executor's private per-node verdict.
#[derive(Clone, Debug)]
pub enum NodeExecutionStatus {
    /// Served from a cache, or left resident by the pre-run cut — available, not recomputed.
    Cached,
    Executed {
        elapsed_secs: f64,
    },
    /// The run could not satisfy these input ports, by index on the node. Never empty:
    /// this status exists because at least one required port went unfed.
    MissingInputs {
        ports: Vec<usize>,
    },
    /// The lambda ran and failed, or the node never ran at all — an errored dependency, a func
    /// with no implementation, or a cached output that no longer loads.
    Errored {
        error: RunError,
    },
}

/// One node's whole result for one run. `status` is `None` for a node the run had nothing
/// to say about that nonetheless holds RAM — a value resident from an earlier run, which
/// this run neither recomputed nor released.
#[derive(Clone, Debug)]
pub struct NodeStatus {
    pub node_id: NodeId,
    pub status: Option<NodeExecutionStatus>,
    pub ram: RamUsage,
}
