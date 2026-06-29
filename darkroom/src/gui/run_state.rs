//! Last graph run's per-node state, keyed by authoring `NodeId`: the
//! execution outcome (status glow + header time), the run's log lines, and
//! — fetched on demand for open inspection panels — the computed runtime
//! input/output values. One [`RunState`] per [`Editor`], rebuilt each run.
//!
//! Execution dissolves subgraphs and remaps interior node ids, so a run's
//! raw `ExecutionStats` are keyed by *flattened* ids. [`RunState::set_results`]
//! uses each stat's `NodeAddr` to fold the outcome back onto the authoring
//! nodes: onto the node itself (`interior`, unique per editor node — a
//! def's interior aggregates across its instances) and onto every ancestor
//! composite instance (`path`), so an instance node reflects its whole
//! subtree. Logs attribute the same way.
//!
//! Values arrive separately and asynchronously: `App` requests them for
//! open panels (see [`RunState::take_requests`]) and feeds replies in via
//! [`RunState::ingest_values`]. A [`RunId`] epoch tags each request/reply
//! so a value computed against a superseded run is dropped; status/logs
//! linger across a re-run (so the glow doesn't blank during compute) while
//! values invalidate immediately.
//!
//! [`Editor`]: crate::gui::app::editor::Editor

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use palantir::Ui;
use scenarium::execution::{ArgumentValues, RunError};
use scenarium::prelude::{ExecutionStats, LogEntry, NodeId, RunPhase, RunProgress};

use crate::core::worker::{RunId, ValueRequest};
use crate::gui::node_values::{NodeValueView, build_view};

/// Per-node execution outcome of the last run. Ordered low→high so a
/// higher-severity status wins when several fold onto one node
/// (`Errored` > `MissingInputs` > `Executed` > `Cached`). `Executed`
/// carries the node's wall-clock run time (seconds). `Running` is the
/// transient live state while a node computes (set directly from
/// `RunProgress`, never produced by the final `set_results`); it carries the
/// instant the node started so the UI can show live elapsed-so-far.
#[derive(Clone, Copy, PartialEq, Default, Debug)]
pub(crate) enum ExecStatus {
    #[default]
    None,
    Cached,
    Executed(f64),
    Running(Instant),
    MissingInputs,
    Errored,
}

impl ExecStatus {
    /// Severity rank, for folding several outcomes onto one editor node
    /// (a subgraph def's interior node runs once per instance; an
    /// instance node aggregates its whole subtree). Higher wins. `Running`
    /// is live-only — it's set directly, never folded through `merged`.
    fn severity(self) -> u8 {
        match self {
            ExecStatus::None => 0,
            ExecStatus::Cached => 1,
            ExecStatus::Executed(_) => 2,
            ExecStatus::Running(_) => 3,
            ExecStatus::MissingInputs => 4,
            ExecStatus::Errored => 5,
        }
    }

    /// Fold two outcomes for the same editor node: two `Executed` times
    /// sum (total compute across instances / subtree); otherwise the
    /// worse status wins.
    fn merged(self, other: ExecStatus) -> ExecStatus {
        match (self, other) {
            (ExecStatus::Executed(a), ExecStatus::Executed(b)) => ExecStatus::Executed(a + b),
            _ if other.severity() >= self.severity() => other,
            _ => self,
        }
    }
}

/// Everything the editor knows about one node from the last run. `values`
/// is `None` until fetched (and only for nodes whose panel is open).
#[derive(Default, Debug)]
pub(crate) struct NodeRunState {
    pub(crate) status: ExecStatus,
    pub(crate) logs: Vec<LogEntry>,
    pub(crate) values: Option<NodeValueView>,
}

/// The last run's per-node state plus value-fetch coordination. Off the
/// serialized state; rebuilt each run.
#[derive(Default, Debug)]
pub(crate) struct RunState {
    nodes: HashMap<NodeId, NodeRunState>,
    /// A run is in flight (`begin_run` → its `ExecutionFinished`). Drives the
    /// live repaint tick and whether the Cancel affordance shows.
    running: bool,
    /// Current run epoch (tags value requests/replies).
    run_id: RunId,
    /// Nodes already asked about this epoch (insert-only; cleared only on
    /// a new epoch / `clear`). Dedups so the frame loop sends one request
    /// per node per run — a reply, including a `None` one, doesn't reopen
    /// the node for re-request.
    requested: HashSet<NodeId>,
}

impl RunState {
    pub(crate) fn status(&self, id: NodeId) -> ExecStatus {
        self.nodes.get(&id).map(|n| n.status).unwrap_or_default()
    }

    /// Whether a run is in flight (kicked, not yet finished/cancelled). Drives
    /// a per-frame repaint so the running node's elapsed-so-far timer keeps
    /// ticking between progress events (a long single node emits none until it
    /// finishes), and gates the Cancel affordance.
    pub(crate) fn is_running(&self) -> bool {
        self.running
    }

    pub(crate) fn logs(&self, id: NodeId) -> &[LogEntry] {
        self.nodes
            .get(&id)
            .map(|n| n.logs.as_slice())
            .unwrap_or(&[])
    }

    pub(crate) fn values(&self, id: NodeId) -> Option<&NodeValueView> {
        self.nodes.get(&id).and_then(|n| n.values.as_ref())
    }

    /// Reproject a finished run's status + logs onto the per-node map.
    /// Status/logs are reset and rebuilt; already-fetched values are kept
    /// (they belong to this epoch — a value reply can land before its
    /// stats). Entries left carrying nothing are dropped.
    pub(crate) fn set_results(&mut self, stats: &ExecutionStats) {
        self.running = false;
        for node in self.nodes.values_mut() {
            node.status = ExecStatus::None;
            node.logs.clear();
        }
        for e in &stats.executed_nodes {
            self.record_status(stats, e.node_id, ExecStatus::Executed(e.elapsed_secs));
        }
        for id in &stats.cached_nodes {
            self.record_status(stats, *id, ExecStatus::Cached);
        }
        for port in &stats.missing_inputs {
            self.record_status(stats, port.node_id, ExecStatus::MissingInputs);
        }
        for e in &stats.node_errors {
            // A node cancelled mid-run didn't fail — it was interrupted and
            // will re-run next time. Leave it neutral (no glow) rather than
            // flagging it as an error.
            if matches!(e.error, RunError::Cancelled { .. }) {
                continue;
            }
            self.record_status(stats, e.node_id, ExecStatus::Errored);
        }
        for entry in &stats.logs {
            for node_id in stats.flatten.attribution(entry.node_id) {
                self.nodes.entry(node_id).or_default().logs.push(LogEntry {
                    node_id,
                    level: entry.level,
                    message: entry.message.clone(),
                });
            }
        }
        self.nodes.retain(|_, n| {
            n.status != ExecStatus::None || !n.logs.is_empty() || n.values.is_some()
        });
    }

    /// Fold one flattened stat's `status` onto the node itself and every
    /// enclosing composite instance (via the flatten map's attribution).
    fn record_status(&mut self, stats: &ExecutionStats, flat_id: NodeId, status: ExecStatus) {
        for node_id in stats.flatten.attribution(flat_id) {
            let slot = self.nodes.entry(node_id).or_default();
            slot.status = slot.status.merged(status);
        }
    }

    /// Apply one live [`RunProgress`] event: mark the node(s) `Running` on
    /// `Started`, `Executed` on `Finished`. Overwrites the prior status (the
    /// final [`set_results`](Self::set_results) reconciles, e.g. summing an
    /// instance subtree's times). `nodes` is already the authoring
    /// attribution, so no flatten projection is needed here.
    pub(crate) fn apply_progress(&mut self, progress: &RunProgress) {
        let status = match progress.phase {
            RunPhase::Started { at } => ExecStatus::Running(at),
            RunPhase::Finished { elapsed_secs } => ExecStatus::Executed(elapsed_secs),
        };
        for &id in &progress.nodes {
            self.nodes.entry(id).or_default().status = status;
        }
    }

    /// Drop everything (a failed run paints no glow and shows no logs /
    /// values).
    pub(crate) fn clear(&mut self) {
        self.running = false;
        self.nodes.clear();
        self.requested.clear();
    }

    /// Open a new value-fetch epoch: a re-run invalidates last run's
    /// values immediately, but keeps status/logs so the glow doesn't blank
    /// during compute (the new run's stats replace them). Pending request
    /// markers reset so open panels re-request under the new epoch.
    pub(crate) fn begin_run(&mut self) {
        self.running = true;
        self.run_id = self.run_id.wrapping_add(1);
        self.requested.clear();
        for node in self.nodes.values_mut() {
            node.values = None;
        }
        self.nodes
            .retain(|_, n| n.status != ExecStatus::None || !n.logs.is_empty());
    }

    /// Deposit a worker value reply (uploading any preview textures via
    /// `ui`). A reply tagged with a stale epoch, or for a node the worker
    /// couldn't resolve (`None`), stores nothing — the node stays "asked"
    /// either way, so it isn't re-requested until the next epoch.
    pub(crate) fn ingest_values(
        &mut self,
        ui: &Ui,
        request: ValueRequest,
        values: Option<ArgumentValues>,
    ) {
        if request.run_id != self.run_id {
            return;
        }
        let Some(values) = values else {
            return;
        };
        self.nodes.entry(request.node_id).or_default().values = Some(build_view(ui, values));
    }

    /// Pending value requests for the `open` panels: each open node not yet
    /// asked about this epoch, marked asked here and returned (tagged with
    /// the current epoch) for `App` to forward to the worker. Keeps all
    /// request bookkeeping on the projection.
    pub(crate) fn take_requests(
        &mut self,
        open: impl Iterator<Item = NodeId>,
    ) -> Vec<ValueRequest> {
        let run_id = self.run_id;
        open.filter(|&node_id| self.requested.insert(node_id))
            .map(|node_id| ValueRequest { node_id, run_id })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scenarium::prelude::{ExecutedNodeStats, FlattenMap, FuncId, LogLevel, NodeError};

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
                    error: RunError::Invoke {
                        func_id: FuncId::from_u128(0),
                        message: "test error".into(),
                    },
                })
                .collect(),
            logs: vec![],
            flatten,
            cancelled: false,
        }
    }

    #[test]
    fn apply_progress_marks_all_attributed_nodes_running_then_executed() {
        let mut rs = RunState::default();
        let (interior, instance) = (nid(1), nid(2));
        let nodes = vec![interior, instance];

        // Started → every attributed node turns Running.
        rs.apply_progress(&RunProgress {
            nodes: nodes.clone(),
            phase: RunPhase::Started {
                at: std::time::Instant::now(),
            },
        });
        assert!(matches!(rs.status(interior), ExecStatus::Running(_)));
        assert!(matches!(rs.status(instance), ExecStatus::Running(_)));

        // Finished → Executed with the reported time (overwrites Running).
        rs.apply_progress(&RunProgress {
            nodes,
            phase: RunPhase::Finished { elapsed_secs: 0.5 },
        });
        assert_eq!(rs.status(interior), ExecStatus::Executed(0.5));
        assert_eq!(rs.status(instance), ExecStatus::Executed(0.5));

        // A node no event mentioned stays None.
        assert_eq!(rs.status(nid(99)), ExecStatus::None);
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

        let mut rs = RunState::default();
        rs.set_results(&stats(map, &[(flat_a, 2.0), (flat_b, 3.0)], &[]));

        // Shared interior view: both instances' times sum (2 + 3).
        assert_eq!(rs.status(interior), ExecStatus::Executed(5.0));
        // Each instance node carries only its own run.
        assert_eq!(rs.status(inst_a), ExecStatus::Executed(2.0));
        assert_eq!(rs.status(inst_b), ExecStatus::Executed(3.0));
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

        let mut rs = RunState::default();
        rs.set_results(&stats(map, &[(flat, 4.0)], &[]));

        assert_eq!(rs.status(interior), ExecStatus::Executed(4.0));
        assert_eq!(rs.status(inner), ExecStatus::Executed(4.0));
        assert_eq!(rs.status(outer), ExecStatus::Executed(4.0));
    }

    /// Worst status wins when a node both executed and errored (the
    /// errored node is in both lists); time is dropped with the upgrade.
    #[test]
    fn errored_beats_executed_on_same_node() {
        let interior = nid(1); // top-level: flattened id == interior
        let mut map = FlattenMap::default();
        map.reset();
        map.set_leaf(interior, 0, interior);

        let mut rs = RunState::default();
        rs.set_results(&stats(map, &[(interior, 1.0)], &[interior]));
        assert_eq!(rs.status(interior), ExecStatus::Errored);
    }

    /// A log line emitted inside a subgraph instance attributes to both
    /// the interior node and the enclosing instance, preserving level +
    /// message, re-keyed to each editor node.
    #[test]
    fn set_results_attributes_logs_to_interior_and_instance() {
        let interior = nid(1);
        let inst = nid(10);
        let flat = nid(100);
        let mut map = FlattenMap::default();
        map.reset();
        let scope = map.push_scope(inst, 0);
        map.set_leaf(flat, scope, interior);

        let mut s = stats(map, &[], &[]);
        s.logs.push(LogEntry {
            node_id: flat,
            level: LogLevel::Warn,
            message: "hi".into(),
        });

        let mut rs = RunState::default();
        rs.set_results(&s);

        let i = rs.logs(interior);
        assert_eq!(i.len(), 1, "interior carries the line");
        assert_eq!(i[0].message, "hi");
        assert_eq!(i[0].level, LogLevel::Warn);
        assert_eq!(i[0].node_id, interior);
        let n = rs.logs(inst);
        assert_eq!(n.len(), 1);
        assert_eq!(n[0].node_id, inst, "re-keyed to the instance");
    }

    /// A new epoch bumps the id and drops values, but keeps status/logs so
    /// the glow survives a recompute; pending markers reset.
    #[test]
    fn begin_run_bumps_id_drops_values_keeps_status() {
        let node = nid(1);
        let mut map = FlattenMap::default();
        map.reset();
        map.set_leaf(node, 0, node);

        let mut rs = RunState::default();
        rs.set_results(&stats(map, &[(node, 1.0)], &[]));
        rs.nodes.get_mut(&node).unwrap().values = Some(NodeValueView::default());
        rs.requested.insert(node);

        assert!(!rs.is_running(), "not running after a finished run");
        rs.begin_run();

        assert!(rs.is_running(), "running once a run is kicked");
        assert_eq!(rs.run_id, 1);
        assert_eq!(rs.status(node), ExecStatus::Executed(1.0), "status lingers");
        assert!(rs.values(node).is_none(), "values invalidated");
        assert!(rs.requested.is_empty(), "pending markers reset");

        // The finishing run clears the in-flight flag.
        rs.set_results(&stats(FlattenMap::default(), &[], &[]));
        assert!(!rs.is_running(), "not running after results land");
    }

    /// `take_requests` asks for each open node once per epoch, then nothing
    /// more — a node already asked is not re-requested even if no value
    /// landed (a `None` reply must not reopen it). A new epoch re-asks.
    #[test]
    fn take_requests_asks_once_per_epoch() {
        let (a, b) = (nid(1), nid(2));
        let req = |node_id, run_id| ValueRequest { node_id, run_id };
        let mut rs = RunState::default();

        // First pass: both asked.
        assert_eq!(
            rs.take_requests([a, b].into_iter()),
            vec![req(a, 0), req(b, 0)]
        );
        // Already asked this epoch → nothing, regardless of whether values
        // arrived. `a` got a value, `b` got nothing (a `None` reply); both
        // stay asked, so neither is re-requested.
        rs.nodes.entry(a).or_default().values = Some(NodeValueView::default());
        assert!(rs.take_requests([a, b].into_iter()).is_empty());

        // A new epoch resets the asked set → both re-asked under run 1.
        rs.begin_run();
        assert_eq!(
            rs.take_requests([a, b].into_iter()),
            vec![req(a, 1), req(b, 1)]
        );
    }
}
