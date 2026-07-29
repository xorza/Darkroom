//! What one completed run reports: one status row per node, plus the whole-run
//! facts that belong to no node.
//!
//! [`NodeStatus`] is the row the executor emits *and* the row the host consumes —
//! [`WorkerStatus`](crate::worker::status::WorkerStatus) publishes it verbatim. A node
//! appears at most once, so nothing downstream has to reassemble it from several lists,
//! and no consumer's fold order can change what a node's result was.

use std::time::Instant;

use crate::RamUsage;
use crate::execution::error::RunError;
use crate::execution::event::EventTrigger;
use crate::execution::identity::ExecutionEventPort;
use crate::execution::identity::ExecutionNodeId;
use crate::execution::log::LogEntry;

/// What one node did this run, at the host boundary — the published projection of the
/// executor's private per-node verdict.
#[derive(Clone, Debug)]
pub enum NodeExecutionStatus {
    /// Live only: the node's lambda is running, since `at`.
    Running {
        at: Instant,
    },
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
    /// `elapsed_secs` is `Some` when the lambda ran and failed, timing the attempt, and
    /// `None` when the node never ran at all — an errored dependency, a func with no
    /// implementation, or a cached output that no longer loads.
    Errored {
        elapsed_secs: Option<f64>,
        error: RunError,
    },
}

/// One node's whole result for one run. `status` is `None` for a node the run had nothing
/// to say about that nonetheless holds RAM — a value resident from an earlier run, which
/// this run neither recomputed nor released.
#[derive(Clone, Debug)]
pub struct NodeStatus {
    pub e_node_id: ExecutionNodeId,
    pub status: Option<NodeExecutionStatus>,
    pub ram: RamUsage,
}

#[derive(Debug, Default)]
pub(crate) struct ExecutionOutcome {
    pub(crate) elapsed_secs: f64,
    /// One row per node with something to report — a status, retained RAM, or both.
    /// Program order, so a node cannot appear twice.
    pub(crate) nodes: Vec<NodeStatus>,
    /// How many nodes invoked their lambda, successes and failures alike. Counted while
    /// the rows are built rather than derived from them, since a failed node's row no
    /// longer says "executed" separately.
    pub(crate) ran_node_count: usize,
    pub(crate) triggered_events: Vec<ExecutionEventPort>,
    pub(crate) event_triggers: Vec<EventTrigger>,
    pub(crate) logs: Vec<LogEntry>,
    pub(crate) cancelled: bool,
    pub(crate) cache_ram: RamUsage,
}

impl ExecutionOutcome {
    pub(crate) fn clear(&mut self) {
        self.elapsed_secs = 0.0;
        self.nodes.clear();
        self.ran_node_count = 0;
        self.triggered_events.clear();
        self.event_triggers.clear();
        self.logs.clear();
        self.cancelled = false;
        self.cache_ram = RamUsage::default();
    }
}

#[cfg(test)]
mod internals {
    use crate::RamUsage;
    use crate::execution::error::RunError;
    use crate::execution::identity::ExecutionNodeId;
    use crate::execution::outcome::{ExecutionOutcome, NodeExecutionStatus};

    impl ExecutionOutcome {
        /// This node's row, or `None` when the run reported nothing for it.
        pub(crate) fn status(&self, e_node_id: ExecutionNodeId) -> Option<&NodeExecutionStatus> {
            self.nodes
                .iter()
                .find(|node| node.e_node_id == e_node_id)?
                .status
                .as_ref()
        }

        /// Whether the node invoked its lambda and succeeded.
        pub(crate) fn ran(&self, e_node_id: ExecutionNodeId) -> bool {
            matches!(
                self.status(e_node_id),
                Some(NodeExecutionStatus::Executed { .. })
            )
        }

        /// Every node that invoked its lambda and succeeded.
        pub(crate) fn ran_nodes(&self) -> impl Iterator<Item = ExecutionNodeId> + '_ {
            self.nodes
                .iter()
                .filter(|node| matches!(node.status, Some(NodeExecutionStatus::Executed { .. })))
                .map(|node| node.e_node_id)
        }

        /// Whether the node was served from a cache instead of recomputing.
        pub(crate) fn cached(&self, e_node_id: ExecutionNodeId) -> bool {
            matches!(self.status(e_node_id), Some(NodeExecutionStatus::Cached))
        }

        /// Every node served from a cache rather than recomputed.
        pub(crate) fn cached_nodes(&self) -> impl Iterator<Item = ExecutionNodeId> + '_ {
            self.nodes
                .iter()
                .filter(|node| matches!(node.status, Some(NodeExecutionStatus::Cached)))
                .map(|node| node.e_node_id)
        }

        /// This node's failure, or `None` when it did not fail.
        pub(crate) fn error(&self, e_node_id: ExecutionNodeId) -> Option<&RunError> {
            match self.status(e_node_id)? {
                NodeExecutionStatus::Errored { error, .. } => Some(error),
                _ => None,
            }
        }

        /// Every node the run reported a failure for.
        pub(crate) fn errored_nodes(&self) -> impl Iterator<Item = ExecutionNodeId> + '_ {
            self.nodes
                .iter()
                .filter(|node| matches!(node.status, Some(NodeExecutionStatus::Errored { .. })))
                .map(|node| node.e_node_id)
        }

        /// The input ports the run could not satisfy on this node, empty when it had none.
        pub(crate) fn missing_input_ports(&self, e_node_id: ExecutionNodeId) -> &[usize] {
            match self.status(e_node_id) {
                Some(NodeExecutionStatus::MissingInputs { ports }) => ports,
                _ => &[],
            }
        }

        /// Every node the run reported a missing input for.
        pub(crate) fn missing_input_nodes(&self) -> impl Iterator<Item = ExecutionNodeId> + '_ {
            self.nodes
                .iter()
                .filter(|node| {
                    matches!(node.status, Some(NodeExecutionStatus::MissingInputs { .. }))
                })
                .map(|node| node.e_node_id)
        }

        /// RAM this node's resident output holds after the run.
        pub(crate) fn node_ram(&self, e_node_id: ExecutionNodeId) -> RamUsage {
            self.nodes
                .iter()
                .find(|node| node.e_node_id == e_node_id)
                .map(|node| node.ram)
                .unwrap_or_default()
        }

        /// Every node still holding RAM after the run.
        pub(crate) fn ram_holding_nodes(&self) -> impl Iterator<Item = ExecutionNodeId> + '_ {
            self.nodes
                .iter()
                .filter(|node| node.ram.total() > 0)
                .map(|node| node.e_node_id)
        }
    }
}
