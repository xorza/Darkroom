//! Classification of a single node's state over an `ExecutionStats`
//! snapshot. Pure projection — no view concerns.

use hashbrown::HashMap;
use scenarium::execution_stats::{ExecutedNodeStats, NodeError};
use scenarium::graph::NodeId;
use scenarium::prelude::ExecutionStats;

#[derive(Debug, Clone, Copy)]
pub enum NodeExecutionInfo<'a> {
    Errored(&'a NodeError),
    MissingInputs,
    Executed(&'a ExecutedNodeStats),
    Cached,
    None,
}

/// Precomputed `NodeId → NodeExecutionInfo` lookup built once per frame.
/// Replaces repeated linear scans over `ExecutionStats` by each node's
/// renderer — O(N) build, O(1) per lookup.
///
/// Precedence (high-to-low) when a node appears in multiple source lists:
/// `Errored` > `MissingInputs` > `Executed` > `Cached`. Matches the
/// order `NodeExecutionInfo::from_stats` used previously.
#[derive(Debug)]
pub struct NodeExecutionIndex<'a> {
    entries: HashMap<NodeId, NodeExecutionInfo<'a>>,
}

impl<'a> NodeExecutionIndex<'a> {
    pub fn new(stats: Option<&'a ExecutionStats>) -> Self {
        let mut entries = HashMap::new();
        let Some(stats) = stats else {
            return Self { entries };
        };

        // Insert low-priority first, overwrite with higher.
        for node_id in &stats.cached_nodes {
            entries.insert(*node_id, NodeExecutionInfo::Cached);
        }
        for executed in &stats.executed_nodes {
            entries.insert(executed.node_id, NodeExecutionInfo::Executed(executed));
        }
        for port in &stats.missing_inputs {
            entries.insert(port.target_id, NodeExecutionInfo::MissingInputs);
        }
        for err in &stats.node_errors {
            entries.insert(err.node_id, NodeExecutionInfo::Errored(err));
        }

        Self { entries }
    }

    pub fn get(&self, node_id: NodeId) -> NodeExecutionInfo<'a> {
        self.entries
            .get(&node_id)
            .copied()
            .unwrap_or(NodeExecutionInfo::None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scenarium::execution_graph::Error;
    use scenarium::execution_stats::{ExecutedNodeStats, NodeError};
    use scenarium::function::FuncId;
    use scenarium::graph::{NodeId, PortAddress};

    fn empty_stats() -> ExecutionStats {
        ExecutionStats {
            elapsed_secs: 0.0,
            executed_nodes: Vec::new(),
            missing_inputs: Vec::new(),
            cached_nodes: Vec::new(),
            triggered_events: Vec::new(),
            node_errors: Vec::new(),
        }
    }

    #[test]
    fn empty_stats_returns_none() {
        let index = NodeExecutionIndex::new(None);
        assert!(matches!(
            index.get(NodeId::unique()),
            NodeExecutionInfo::None
        ));
    }

    #[test]
    fn errored_overrides_executed_and_cached() {
        let id = NodeId::unique();
        let mut stats = empty_stats();
        stats.cached_nodes.push(id);
        stats.executed_nodes.push(ExecutedNodeStats {
            node_id: id,
            elapsed_secs: 0.0,
        });
        stats.node_errors.push(NodeError {
            node_id: id,
            error: Error::Invoke {
                func_id: FuncId::unique(),
                message: "boom".into(),
            },
        });

        let index = NodeExecutionIndex::new(Some(&stats));
        assert!(matches!(index.get(id), NodeExecutionInfo::Errored(_)));
    }

    #[test]
    fn missing_inputs_overrides_executed() {
        let id = NodeId::unique();
        let mut stats = empty_stats();
        stats.executed_nodes.push(ExecutedNodeStats {
            node_id: id,
            elapsed_secs: 0.0,
        });
        stats.missing_inputs.push(PortAddress {
            target_id: id,
            port_idx: 0,
        });

        let index = NodeExecutionIndex::new(Some(&stats));
        assert!(matches!(index.get(id), NodeExecutionInfo::MissingInputs));
    }

    #[test]
    fn executed_overrides_cached() {
        let id = NodeId::unique();
        let mut stats = empty_stats();
        stats.cached_nodes.push(id);
        stats.executed_nodes.push(ExecutedNodeStats {
            node_id: id,
            elapsed_secs: 1.5,
        });

        let index = NodeExecutionIndex::new(Some(&stats));
        match index.get(id) {
            NodeExecutionInfo::Executed(e) => assert_eq!(e.elapsed_secs, 1.5),
            other => panic!("expected Executed, got {other:?}"),
        }
    }

    #[test]
    fn cached_only() {
        let id = NodeId::unique();
        let mut stats = empty_stats();
        stats.cached_nodes.push(id);
        let index = NodeExecutionIndex::new(Some(&stats));
        assert!(matches!(index.get(id), NodeExecutionInfo::Cached));
    }

    #[test]
    fn absent_node_returns_none() {
        let mut stats = empty_stats();
        stats.cached_nodes.push(NodeId::unique());
        let index = NodeExecutionIndex::new(Some(&stats));
        assert!(matches!(
            index.get(NodeId::unique()),
            NodeExecutionInfo::None
        ));
    }
}
