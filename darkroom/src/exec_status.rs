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
/// (its `interior` id) and onto every enclosing composite instance (via
/// the flatten map's `attribution`). `out` is cleared first; keys absent
/// from the map paint no glow.
pub(crate) fn project_stats(out: &mut HashMap<NodeId, ExecStatus>, stats: &ExecutionStats) {
    out.clear();
    let mut rec = |flat_id: NodeId, status: ExecStatus| {
        for node_id in stats.flatten.attribution(flat_id) {
            let slot = out.entry(node_id).or_default();
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
    use scenarium::prelude::{ExecutedNodeStats, FlattenMap, NodeError};

    fn nid(n: u128) -> NodeId {
        NodeId::from_u128(n)
    }

    /// Build an `ExecutionStats` carrying `flatten`, with the given
    /// executed `(flat_id, secs)` and errored `flat_id`s.
    fn stats(
        flatten: FlattenMap,
        executed: &[(NodeId, f64)],
        errored: &[NodeId],
    ) -> ExecutionStats {
        ExecutionStats {
            elapsed_secs: 0.0,
            executed_nodes: executed
                .iter()
                .map(|&(node_id, elapsed_secs)| ExecutedNodeStats {
                    node_id,
                    elapsed_secs,
                })
                .collect(),
            missing_inputs: vec![],
            cached_nodes: vec![],
            triggered_events: vec![],
            node_errors: errored
                .iter()
                .map(|&node_id| NodeError {
                    node_id,
                    error: scenarium::execution::Error::CycleDetected { node_id },
                })
                .collect(),
            flatten,
        }
    }

    /// Two instances of one def → the interior node's flattened ids both
    /// fold onto its authoring `interior` id, summing time; the instance
    /// nodes each get only their own run.
    #[test]
    fn aggregates_interior_across_instances_and_per_instance_subtree() {
        let interior = nid(1);
        let (inst_a, inst_b) = (nid(10), nid(20));
        let (flat_a, flat_b) = (nid(101), nid(102));

        let mut map = FlattenMap::default();
        map.reset();
        let scope_a = map.push_scope(inst_a, 0);
        let scope_b = map.push_scope(inst_b, 0);
        map.set_leaf(flat_a, scope_a, interior);
        map.set_leaf(flat_b, scope_b, interior);

        let mut out = HashMap::new();
        project_stats(&mut out, &stats(map, &[(flat_a, 2.0), (flat_b, 3.0)], &[]));

        // Shared interior view: both instances' times sum (2 + 3).
        assert_eq!(out.get(&interior), Some(&ExecStatus::Executed(5.0)));
        // Each instance node carries only its own run.
        assert_eq!(out.get(&inst_a), Some(&ExecStatus::Executed(2.0)));
        assert_eq!(out.get(&inst_b), Some(&ExecStatus::Executed(3.0)));
    }

    /// A node nested two levels deep accumulates onto *both* enclosing
    /// instances — the outer instance's total includes nested cost.
    #[test]
    fn outer_instance_total_includes_nested() {
        let interior = nid(1);
        let (outer, inner) = (nid(10), nid(20));
        let flat = nid(100);

        let mut map = FlattenMap::default();
        map.reset();
        let scope_outer = map.push_scope(outer, 0);
        let scope_inner = map.push_scope(inner, scope_outer);
        map.set_leaf(flat, scope_inner, interior);

        let mut out = HashMap::new();
        project_stats(&mut out, &stats(map, &[(flat, 4.0)], &[]));

        assert_eq!(out.get(&interior), Some(&ExecStatus::Executed(4.0)));
        assert_eq!(out.get(&inner), Some(&ExecStatus::Executed(4.0)));
        assert_eq!(out.get(&outer), Some(&ExecStatus::Executed(4.0)));
    }

    /// Worst status wins when a node both executed and errored (the
    /// errored node is in both lists); time is dropped with the upgrade.
    #[test]
    fn errored_beats_executed_on_same_node() {
        let interior = nid(1); // top-level: flattened id == interior
        let mut map = FlattenMap::default();
        map.reset();
        map.set_leaf(interior, 0, interior);

        let mut out = HashMap::new();
        project_stats(&mut out, &stats(map, &[(interior, 1.0)], &[interior]));
        assert_eq!(out.get(&interior), Some(&ExecStatus::Errored));
    }
}
