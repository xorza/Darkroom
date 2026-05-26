//! Per-node execution outcome and the projection that maps a run's
//! `ExecutionStats` back onto the editor's nodes.
//!
//! Execution dissolves subgraphs and remaps interior node ids, so the
//! raw stats are keyed by *flattened* ids. `project_stats` uses each
//! node's `NodeAddr` to fold the outcome back onto the authoring nodes:
//! onto the node itself (`interior`, unique per editor node — a def's
//! interior aggregates across its instances) and onto every ancestor
//! composite instance (`path`), so an instance node reflects its whole
//! subtree. The result feeds `SceneNode::exec_status` (the status glow +
//! header time label).

use std::collections::HashMap;

use scenarium::prelude::{ExecutionStats, NodeId};

/// Per-node outcome of the last graph run. Ordered low→high so a
/// higher-severity status wins when several fold onto one node
/// (`Errored` > `MissingInputs` > `Executed` > `Cached`) — mirrors the
/// deprecated editor's precedence. `Executed` carries the node's
/// wall-clock run time (seconds).
#[derive(Clone, Copy, PartialEq, Default, Debug)]
pub enum ExecStatus {
    #[default]
    None,
    Cached,
    Executed(f64),
    MissingInputs,
    Errored,
}

impl ExecStatus {
    /// Severity rank, for folding several outcomes onto one editor node
    /// (a subgraph def's interior node runs once per instance; an
    /// instance node aggregates its whole subtree). Higher wins.
    fn severity(self) -> u8 {
        match self {
            ExecStatus::None => 0,
            ExecStatus::Cached => 1,
            ExecStatus::Executed(_) => 2,
            ExecStatus::MissingInputs => 3,
            ExecStatus::Errored => 4,
        }
    }

    /// Fold two outcomes for the same editor node: two `Executed` times
    /// sum (total compute across instances / subtree); otherwise the
    /// worse status wins.
    pub(crate) fn merged(self, other: ExecStatus) -> ExecStatus {
        match (self, other) {
            (ExecStatus::Executed(a), ExecStatus::Executed(b)) => ExecStatus::Executed(a + b),
            _ if other.severity() >= self.severity() => other,
            _ => self,
        }
    }
}

/// Project a run's stats onto an editor-`NodeId`-keyed status map. Each
/// flattened stat folds (via [`ExecStatus::merged`]) onto the node itself
/// (`addr.interior`) and onto every ancestor composite instance
/// (`addr.path`). `out` is cleared first; keys absent from the map paint
/// no glow.
pub(crate) fn project_stats(out: &mut HashMap<NodeId, ExecStatus>, stats: &ExecutionStats) {
    out.clear();
    let mut rec = |flat_id: NodeId, status: ExecStatus| {
        let Some(addr) = stats.addrs.get(&flat_id) else {
            return;
        };
        for key in std::iter::once(&addr.interior).chain(addr.path.iter()) {
            let slot = out.entry(*key).or_default();
            *slot = slot.merged(status);
        }
    };
    for e in &stats.executed_nodes {
        rec(e.node_id, ExecStatus::Executed(e.elapsed_secs));
    }
    for id in &stats.cached_nodes {
        rec(*id, ExecStatus::Cached);
    }
    for port in &stats.missing_inputs {
        rec(port.node_id, ExecStatus::MissingInputs);
    }
    for e in &stats.node_errors {
        rec(e.node_id, ExecStatus::Errored);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scenarium::prelude::{ExecutedNodeStats, NodeAddr};

    fn nid(n: u128) -> NodeId {
        NodeId::from_u128(n)
    }

    /// Two instances of one def → the interior node's flattened ids both
    /// fold onto its authoring `interior` id, summing time; the instance
    /// nodes each get only their own subtree.
    #[test]
    fn aggregates_interior_across_instances_and_per_instance_subtree() {
        let interior = nid(1);
        let inst_a = nid(10);
        let inst_b = nid(20);
        // Two flattened runs of the same interior node, one per instance.
        let flat_a = nid(101);
        let flat_b = nid(102);
        let mut addrs = HashMap::new();
        addrs.insert(
            flat_a,
            NodeAddr {
                path: vec![inst_a],
                interior,
            },
        );
        addrs.insert(
            flat_b,
            NodeAddr {
                path: vec![inst_b],
                interior,
            },
        );
        let stats = ExecutionStats {
            elapsed_secs: 0.0,
            executed_nodes: vec![
                ExecutedNodeStats {
                    node_id: flat_a,
                    elapsed_secs: 2.0,
                },
                ExecutedNodeStats {
                    node_id: flat_b,
                    elapsed_secs: 3.0,
                },
            ],
            missing_inputs: vec![],
            cached_nodes: vec![],
            triggered_events: vec![],
            node_errors: vec![],
            addrs,
        };

        let mut out = HashMap::new();
        project_stats(&mut out, &stats);

        // Shared interior view: both instances' times sum (2 + 3).
        assert_eq!(out.get(&interior), Some(&ExecStatus::Executed(5.0)));
        // Each instance node carries only its own run.
        assert_eq!(out.get(&inst_a), Some(&ExecStatus::Executed(2.0)));
        assert_eq!(out.get(&inst_b), Some(&ExecStatus::Executed(3.0)));
    }

    /// Worst status wins when a node both executed and errored (the
    /// errored node is in both lists); time is dropped with the upgrade.
    #[test]
    fn errored_beats_executed_on_same_node() {
        let interior = nid(1);
        let flat = nid(1); // top-level: flattened id == interior
        let mut addrs = HashMap::new();
        addrs.insert(
            flat,
            NodeAddr {
                path: vec![],
                interior,
            },
        );
        let stats = ExecutionStats {
            elapsed_secs: 0.0,
            executed_nodes: vec![ExecutedNodeStats {
                node_id: flat,
                elapsed_secs: 1.0,
            }],
            missing_inputs: vec![],
            cached_nodes: vec![],
            triggered_events: vec![],
            node_errors: vec![scenarium::prelude::NodeError {
                node_id: flat,
                error: scenarium::execution::Error::CycleDetected { node_id: flat },
            }],
            addrs,
        };

        let mut out = HashMap::new();
        project_stats(&mut out, &stats);
        assert_eq!(out.get(&interior), Some(&ExecStatus::Errored));
    }
}
