//! Classification of a single node's state over an `ExecutionStats`
//! snapshot. Pure projection — no view concerns.

use scenarium::execution_stats::{ExecutedNodeStats, NodeError};
use scenarium::graph::NodeId;
use scenarium::prelude::ExecutionStats;

#[derive(Debug)]
pub enum NodeExecutionInfo<'a> {
    Errored(&'a NodeError),
    MissingInputs,
    Executed(&'a ExecutedNodeStats),
    Cached,
    None,
}

impl<'a> NodeExecutionInfo<'a> {
    pub fn from_stats(stats: Option<&'a ExecutionStats>, node_id: NodeId) -> Self {
        let Some(stats) = stats else {
            return Self::None;
        };

        if let Some(err) = stats.node_errors.iter().find(|e| e.node_id == node_id) {
            return Self::Errored(err);
        }

        if stats.missing_inputs.iter().any(|p| p.target_id == node_id) {
            return Self::MissingInputs;
        }

        if let Some(executed) = stats.executed_nodes.iter().find(|s| s.node_id == node_id) {
            return Self::Executed(executed);
        }

        if stats.cached_nodes.contains(&node_id) {
            return Self::Cached;
        }

        Self::None
    }
}
