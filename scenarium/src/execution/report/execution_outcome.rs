//! [`ExecutionOutcome`]: what a completed run hands back.

use crate::data::ram_usage::RamUsage;
use crate::execution::report::event_trigger::EventTrigger;
use crate::execution::report::node_status::NodeStatus;
use crate::runtime::log_entry::LogEntry;

#[derive(Debug, Default)]
pub(crate) struct ExecutionOutcome {
    /// One row per node with something to report — a status, retained RAM, or both.
    /// `CompiledGraph` order, so a node cannot appear twice.
    pub(crate) nodes: Vec<NodeStatus>,
    /// How many nodes invoked their lambda, successes and failures alike. Counted while
    /// the rows are built rather than derived from them, since a failed node's row no
    /// longer says "executed" separately.
    pub(crate) executed_node_count: usize,
    pub(crate) event_triggers: Vec<EventTrigger>,
    pub(crate) logs: Vec<LogEntry>,
    pub(crate) cancelled: bool,
    pub(crate) cache_ram: RamUsage,
}

impl ExecutionOutcome {
    pub(crate) fn clear(&mut self) {
        self.nodes.clear();
        self.executed_node_count = 0;
        self.event_triggers.clear();
        self.logs.clear();
        self.cancelled = false;
        self.cache_ram = RamUsage::default();
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use crate::execution::error::RunError;
    use crate::execution::report::execution_outcome::ExecutionOutcome;
    use crate::execution::report::node_status::NodeExecutionStatus;
    use crate::graph::identity::NodeId;

    impl ExecutionOutcome {
        /// This node's row, or `None` when the run reported nothing for it.
        pub(crate) fn status(&self, node_id: NodeId) -> Option<&NodeExecutionStatus> {
            self.nodes
                .iter()
                .find(|node| node.node_id == node_id)?
                .status
                .as_ref()
        }

        /// Whether the node invoked its lambda and succeeded.
        pub(crate) fn ran(&self, node_id: NodeId) -> bool {
            matches!(
                self.status(node_id),
                Some(NodeExecutionStatus::Executed { .. })
            )
        }

        /// Whether the node was served from a cache instead of recomputing.
        pub(crate) fn cached(&self, node_id: NodeId) -> bool {
            matches!(self.status(node_id), Some(NodeExecutionStatus::Cached))
        }

        /// This node's failure, or `None` when it did not fail.
        pub(crate) fn error(&self, node_id: NodeId) -> Option<&RunError> {
            match self.status(node_id)? {
                NodeExecutionStatus::Errored { error, .. } => Some(error),
                _ => None,
            }
        }
    }
}
