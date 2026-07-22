//! Last graph run's centralized runtime state: per-node execution outcomes
//! and logs, plus the latest worker-pushed values for pinned outputs. One
//! [`RunState`] per [`Editor`], updated as worker reports arrive.
//!
//! Execution dissolves graphs and remaps interior node ids, so a run's
//! raw `ExecutionStats` are keyed by *flattened* ids. [`RunState::set_results`]
//! projects them through the worker-confirmed [`CompiledGraph`] to fold each outcome
//! onto the authoring nodes: onto the node itself (unique per editor
//! node — a graph's interior aggregates across its instances) and onto every
//! ancestor composite instance, so an instance node reflects its whole
//! subtree. Logs attribute the same way.
//!
//! [`Editor`]: crate::gui::app::editor::Editor

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use aperture::Ui;
use scenarium::CompiledGraph;
use scenarium::ExecutionNodeId;
use scenarium::LogLevel;
use scenarium::NodeId;
use scenarium::RamUsage;
use scenarium::RunError;
use scenarium::{ExecutionStats, PinnedOutputs, RunPhase, RunProgress};

use crate::core::document::Document;
use crate::gui::pinned_output::PinnedOutputStore;

fn attributed_nodes(
    compiled: &CompiledGraph,
    e_node_id: ExecutionNodeId,
) -> impl Iterator<Item = NodeId> + '_ {
    compiled
        .attribution(e_node_id)
        .expect("worker report identity must belong to the acknowledged compiled graph")
}

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
    /// (a graph's interior node runs once per instance; an
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

/// Everything the editor knows about one node from the last run.
#[derive(Default, Debug)]
pub(crate) struct NodeRunState {
    pub(crate) status: ExecStatus,
    pub(crate) logs: Vec<NodeLog>,
    /// Human-readable messages for this run's failures, folded on the same
    /// attribution as `status` (a graph instance collects its subtree's).
    /// Empty unless the node errored; drives the inspector's error detail so
    /// a failed node reads e.g. "no light frames provided", not just "errored".
    pub(crate) errors: Vec<String>,
    /// RAM this node's cached output holds after the last run (system vs GPU),
    /// summed across its flattened contributors — a graph instance aggregates
    /// its interior. Zero unless the node retains a value; drives the node body's
    /// memory readout.
    pub(crate) ram: RamUsage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NodeLog {
    pub(crate) level: LogLevel,
    pub(crate) message: String,
}

/// Central runtime state for the current editor. Off the serialized state.
#[derive(Default, Debug)]
pub(crate) struct RunState {
    nodes: HashMap<NodeId, NodeRunState>,
    pub(crate) pinned_outputs: PinnedOutputStore,
    /// The program acknowledged by the worker's ordered report stream. Every
    /// subsequent flat progress/result payload belongs to this exact compile.
    pub(crate) compiled: Option<Arc<CompiledGraph>>,
    /// A run is in flight (`begin_run` → `WorkerReport::Finished`). Drives the
    /// live repaint tick and whether the Cancel affordance shows.
    running: bool,
    /// RAM held by the worker's runtime cache after the last finished run
    /// (system RAM vs GPU VRAM), mirrored from its `ExecutionStats`. Drives the
    /// status bar's memory readout.
    pub(crate) cache_ram: RamUsage,
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

    pub(crate) fn logs(&self, id: NodeId) -> &[NodeLog] {
        self.nodes
            .get(&id)
            .map(|n| n.logs.as_slice())
            .unwrap_or(&[])
    }

    /// This run's failure messages for a node (the errored node itself, or a
    /// composite instance aggregating its subtree). Empty unless it errored.
    pub(crate) fn errors(&self, id: NodeId) -> &[String] {
        self.nodes
            .get(&id)
            .map(|n| n.errors.as_slice())
            .unwrap_or(&[])
    }

    /// RAM this node's cached output holds after the last run (zero if it holds
    /// nothing). Read into the scene each rebuild to drive the node body's
    /// memory readout.
    pub(crate) fn ram(&self, id: NodeId) -> RamUsage {
        self.nodes.get(&id).map(|n| n.ram).unwrap_or_default()
    }

    /// Reproject a finished run's status + logs onto the per-node map.
    /// Status/logs are reset and rebuilt. Entries left carrying nothing are
    /// dropped. The worker-confirmed compiled graph projects the stats' flat
    /// ids onto authoring nodes.
    pub(crate) fn set_results(&mut self, stats: &ExecutionStats) {
        let compiled = Arc::clone(
            self.compiled
                .as_ref()
                .expect("worker reported results before installing a compiled graph"),
        );
        self.running = false;
        self.cache_ram = stats.cache_ram;
        for node in self.nodes.values_mut() {
            node.status = ExecStatus::None;
            node.logs.clear();
            node.errors.clear();
            node.ram = RamUsage::default();
        }
        for e in &stats.executed_nodes {
            self.record_status(&compiled, e.e_node_id, ExecStatus::Executed(e.elapsed_secs));
        }
        for id in &stats.cached_nodes {
            self.record_status(&compiled, *id, ExecStatus::Cached);
        }
        for port in &stats.missing_inputs {
            self.record_status(&compiled, port.e_node_id, ExecStatus::MissingInputs);
        }
        for e in &stats.node_errors {
            // A node cancelled mid-run didn't fail — it was interrupted and
            // will re-run next time. Leave it neutral (no glow) rather than
            // flagging it as an error.
            if matches!(e.error, RunError::Cancelled { .. }) {
                continue;
            }
            self.record_status(&compiled, e.e_node_id, ExecStatus::Errored);
            self.record_error(&compiled, e.e_node_id, &e.error);
        }
        for entry in &stats.logs {
            for node_id in attributed_nodes(&compiled, entry.e_node_id) {
                self.nodes.entry(node_id).or_default().logs.push(NodeLog {
                    level: entry.level,
                    message: entry.message.clone(),
                });
            }
        }
        // Per-node RAM folds like elapsed time: a flattened node's footprint adds
        // onto its authoring node and every enclosing composite instance, so an
        // instance aggregates its interior's memory.
        for node_ram in &stats.node_ram {
            for node_id in attributed_nodes(&compiled, node_ram.e_node_id) {
                self.nodes.entry(node_id).or_default().ram += node_ram.usage;
            }
        }
        self.nodes
            .retain(|_, n| n.status != ExecStatus::None || !n.logs.is_empty() || n.ram.total() > 0);
    }

    /// Fold one flattened stat's `status` onto the node itself and every
    /// enclosing composite instance (via the flatten map's attribution).
    fn record_status(
        &mut self,
        compiled: &CompiledGraph,
        e_node_id: ExecutionNodeId,
        status: ExecStatus,
    ) {
        for node_id in attributed_nodes(compiled, e_node_id) {
            let slot = self.nodes.entry(node_id).or_default();
            slot.status = slot.status.merged(status);
        }
    }

    /// Fold one run error's message onto the errored node and every enclosing
    /// composite instance (same attribution as `record_status`), so the
    /// inspector can show the actual failure cause instead of a bare "errored".
    /// A graph instance accumulates its whole subtree's failures.
    fn record_error(
        &mut self,
        compiled: &CompiledGraph,
        e_node_id: ExecutionNodeId,
        error: &RunError,
    ) {
        let message = error.to_string();
        for node_id in attributed_nodes(compiled, e_node_id) {
            self.nodes
                .entry(node_id)
                .or_default()
                .errors
                .push(message.clone());
        }
    }

    /// Apply one live [`RunProgress`] event: mark the node(s) `Running` on
    /// `Started`, `Executed` on `Finished`. Overwrites the prior status (the
    /// final [`set_results`](Self::set_results) reconciles, e.g. summing an
    /// instance subtree's times) — deliberately *not* folded through
    /// `record_status`'s `merged`, since `Running` is live-only and must
    /// always win over a stale `Errored`/`MissingInputs` from the last run.
    /// The installed compile is the program the event came from (like
    /// [`set_results`](Self::set_results)); `progress.e_node_id` is a flattened
    /// id projected onto authoring nodes here.
    pub(crate) fn apply_progress(&mut self, progress: &RunProgress) {
        let compiled = Arc::clone(
            self.compiled
                .as_ref()
                .expect("worker reported progress before installing a compiled graph"),
        );
        let status = match progress.phase {
            RunPhase::Started { at } => ExecStatus::Running(at),
            RunPhase::Finished { elapsed_secs } => ExecStatus::Executed(elapsed_secs),
        };
        for node_id in attributed_nodes(&compiled, progress.e_node_id) {
            self.nodes.entry(node_id).or_default().status = status;
        }
    }

    pub(crate) fn ingest_pinned_outputs(
        &mut self,
        ui: &Ui,
        pushed: PinnedOutputs,
        document: &Document,
    ) {
        let compiled = self
            .compiled
            .as_ref()
            .expect("worker pushed outputs before installing a compiled graph");
        let node_id = compiled
            .leaf(pushed.e_node_id)
            .expect("pinned output identity must belong to the installed compile");
        self.pinned_outputs
            .ingest(ui, node_id, pushed.values, document);
    }

    /// Drop everything visible from a failed run: no glow, logs, or pinned
    /// values.
    pub(crate) fn clear(&mut self) {
        self.running = false;
        self.nodes.clear();
        self.pinned_outputs.entries.clear();
    }

    /// Mark a fresh run in flight while keeping status/logs/pinned values so
    /// the glow and pinned previews don't blank during compute; the new run's
    /// stats and pushes replace them as they arrive.
    pub(crate) fn begin_run(&mut self) {
        self.running = true;
        self.nodes
            .retain(|_, n| n.status != ExecStatus::None || !n.logs.is_empty());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use scenarium::CompiledGraphBuilder;
    use scenarium::FuncId;
    use scenarium::{DynamicValue, ExecutedNodeStats, LogEntry, LogLevel, NodeError};
    use scenarium::{OutputPort, PinnedOutput, StaticValue};

    use crate::gui::pinned_output::StoredContent;

    fn nid(n: u128) -> NodeId {
        NodeId::from_u128(n)
    }

    fn eid(n: u128) -> ExecutionNodeId {
        ExecutionNodeId::from_u128(n)
    }

    /// Build an `ExecutionStats` with the given executed `(e_node_id, secs)`
    /// and errored `e_node_id`s. The installed compiled graph isn't part of the
    /// stats; `RunState` retains it from the preceding worker report.
    fn stats(executed: &[(ExecutionNodeId, f64)], errored: &[ExecutionNodeId]) -> ExecutionStats {
        ExecutionStats {
            elapsed_secs: 0.0,
            executed_nodes: executed
                .iter()
                .map(|&(e_node_id, elapsed_secs)| ExecutedNodeStats {
                    e_node_id,
                    elapsed_secs,
                })
                .collect(),
            missing_inputs: vec![],
            cached_nodes: vec![],
            triggered_events: vec![],
            node_errors: errored
                .iter()
                .map(|&e_node_id| NodeError {
                    e_node_id,
                    error: RunError::Invoke {
                        func_id: FuncId::from_u128(0),
                        message: "test error".into(),
                    },
                })
                .collect(),
            logs: vec![],
            cancelled: false,
            cache_ram: RamUsage::default(),
            node_ram: vec![],
        }
    }

    fn run_state(
        leaves: impl IntoIterator<Item = (ExecutionNodeId, Vec<NodeId>, NodeId)>,
    ) -> RunState {
        let mut builder = CompiledGraphBuilder::new();
        for (e_node_id, instances, node_id) in leaves {
            builder.insert_leaf(e_node_id, instances, node_id);
        }
        RunState {
            compiled: Some(builder.build()),
            ..RunState::default()
        }
    }

    #[test]
    fn apply_progress_marks_all_attributed_nodes_running_then_executed() {
        let (interior, instance) = (nid(1), nid(2));
        let e_node_id = eid(100);
        let mut rs = run_state([(e_node_id, vec![instance], interior)]);

        // Started → every attributed node turns Running.
        rs.apply_progress(&RunProgress {
            e_node_id,
            phase: RunPhase::Started { at: Instant::now() },
        });
        assert!(matches!(rs.status(interior), ExecStatus::Running(_)));
        assert!(matches!(rs.status(instance), ExecStatus::Running(_)));

        // Finished → Executed with the reported time (overwrites Running).
        rs.apply_progress(&RunProgress {
            e_node_id,
            phase: RunPhase::Finished { elapsed_secs: 0.5 },
        });
        assert_eq!(rs.status(interior), ExecStatus::Executed(0.5));
        assert_eq!(rs.status(instance), ExecStatus::Executed(0.5));

        // A node no event mentioned stays None.
        assert_eq!(rs.status(nid(99)), ExecStatus::None);
    }

    /// Two instances of one graph → the interior node's flattened ids both
    /// fold onto its authoring `interior` id, summing time; the instance
    /// nodes each get only their own run.
    #[test]
    fn aggregates_interior_across_instances_and_per_instance_subtree() {
        let interior = nid(1);
        let (inst_a, inst_b) = (nid(10), nid(20));
        let (e_node_id_a, e_node_id_b) = (eid(101), eid(102));

        let mut rs = run_state([
            (e_node_id_a, vec![inst_a], interior),
            (e_node_id_b, vec![inst_b], interior),
        ]);
        rs.set_results(&stats(&[(e_node_id_a, 2.0), (e_node_id_b, 3.0)], &[]));

        // Shared interior view: both instances' times sum (2 + 3).
        assert_eq!(rs.status(interior), ExecStatus::Executed(5.0));
        // Each instance node carries only its own run.
        assert_eq!(rs.status(inst_a), ExecStatus::Executed(2.0));
        assert_eq!(rs.status(inst_b), ExecStatus::Executed(3.0));
    }

    #[test]
    fn pinned_outputs_project_execution_occurrences_to_the_authored_port() {
        let interior = nid(1);
        let (instance_a, instance_b) = (nid(10), nid(20));
        let (e_node_id_a, e_node_id_b) = (eid(101), eid(102));
        let mut run_state = run_state([
            (e_node_id_a, vec![instance_a], interior),
            (e_node_id_b, vec![instance_b], interior),
        ]);
        let port = OutputPort::new(interior, 0);
        let mut document = Document::default();
        document.graph.set_output_pinned(port, true);
        let ui = Ui::default();

        for (e_node_id, value) in [(e_node_id_a, 7), (e_node_id_b, 8)] {
            run_state.ingest_pinned_outputs(
                &ui,
                PinnedOutputs {
                    e_node_id,
                    values: vec![PinnedOutput {
                        port_idx: 0,
                        value: DynamicValue::Static(StaticValue::Int(value)),
                    }],
                },
                &document,
            );
        }

        assert_eq!(run_state.pinned_outputs.entries.len(), 1);
        assert!(matches!(
            &run_state.pinned_outputs.entries[&port],
            StoredContent::Text(text) if text == "8"
        ));
    }

    /// A node nested two levels deep accumulates onto *both* enclosing
    /// instances — the outer instance's total includes nested cost.
    #[test]
    fn outer_instance_total_includes_nested() {
        let interior = nid(1);
        let (outer, inner) = (nid(10), nid(20));
        let e_node_id = eid(100);

        let mut rs = run_state([(e_node_id, vec![outer, inner], interior)]);
        rs.set_results(&stats(&[(e_node_id, 4.0)], &[]));

        assert_eq!(rs.status(interior), ExecStatus::Executed(4.0));
        assert_eq!(rs.status(inner), ExecStatus::Executed(4.0));
        assert_eq!(rs.status(outer), ExecStatus::Executed(4.0));
    }

    /// Worst status wins when a node both executed and errored (the
    /// errored node is in both lists); time is dropped with the upgrade.
    #[test]
    fn errored_beats_executed_on_same_node() {
        let interior = nid(1);
        let e_node_id = eid(1);
        let mut rs = run_state([(e_node_id, vec![], interior)]);
        rs.set_results(&stats(&[(e_node_id, 1.0)], &[e_node_id]));
        assert_eq!(rs.status(interior), ExecStatus::Errored);
        // The failure message rides along with the status — the inspector
        // shows it instead of a bare "errored".
        assert_eq!(rs.errors(interior), ["test error"]);
    }

    /// A failure's message folds onto the errored interior node *and* its
    /// enclosing graph instance (same attribution as the status), while a
    /// `Cancelled` "error" records neither status nor message (a cancel is not
    /// a failure).
    #[test]
    fn error_messages_attribute_to_instance_and_skip_cancelled() {
        let interior = nid(1);
        let cancelled_interior = nid(2);
        let inst = nid(10);
        let (fail_e_node_id, cancel_e_node_id) = (eid(100), eid(101));
        let mut rs = run_state([
            (fail_e_node_id, vec![inst], interior),
            (cancel_e_node_id, vec![inst], cancelled_interior),
        ]);

        let node_err = |e_node_id, error| NodeError { e_node_id, error };
        let mut s = stats(&[], &[]);
        s.node_errors = vec![
            node_err(
                fail_e_node_id,
                RunError::Invoke {
                    func_id: FuncId::from_u128(0),
                    message: "no light frames provided".into(),
                },
            ),
            node_err(
                cancel_e_node_id,
                RunError::Cancelled {
                    func_id: FuncId::from_u128(0),
                },
            ),
        ];

        rs.set_results(&s);

        // The real failure paints both the interior and the enclosing instance.
        assert_eq!(rs.status(interior), ExecStatus::Errored);
        assert_eq!(rs.errors(interior), ["no light frames provided"]);
        assert_eq!(rs.status(inst), ExecStatus::Errored);
        assert_eq!(rs.errors(inst), ["no light frames provided"]);
        // The cancelled node contributes no extra status/message (only the one
        // real failure's message is present, not a second entry for the cancel).
        assert_eq!(rs.errors(interior).len(), 1, "cancel adds no message");
        assert_eq!(rs.status(cancelled_interior), ExecStatus::None);
        assert!(rs.errors(cancelled_interior).is_empty());
    }

    /// A log line emitted inside a graph instance attributes to both
    /// the interior node and the enclosing instance, preserving level +
    /// message in each editor node's own state.
    #[test]
    fn set_results_attributes_logs_to_interior_and_instance() {
        let interior = nid(1);
        let inst = nid(10);
        let e_node_id = eid(100);
        let mut rs = run_state([(e_node_id, vec![inst], interior)]);

        let mut s = stats(&[], &[]);
        s.logs.push(LogEntry {
            e_node_id,
            level: LogLevel::Warn,
            message: "hi".into(),
        });

        rs.set_results(&s);

        let i = rs.logs(interior);
        assert_eq!(i.len(), 1, "interior carries the line");
        assert_eq!(i[0].message, "hi");
        assert_eq!(i[0].level, LogLevel::Warn);
        let n = rs.logs(inst);
        assert_eq!(n.len(), 1);
        assert_eq!(n[0], i[0], "both attributed nodes carry the same line");
    }

    /// A fresh run keeps status/logs so the glow survives a recompute.
    #[test]
    fn begin_run_keeps_status() {
        let node = nid(1);
        let e_node_id = eid(1);
        let mut rs = run_state([(e_node_id, vec![], node)]);
        rs.set_results(&stats(&[(e_node_id, 1.0)], &[]));

        assert!(!rs.is_running(), "not running after a finished run");
        rs.begin_run();

        assert!(rs.is_running(), "running once a run is kicked");
        assert_eq!(rs.status(node), ExecStatus::Executed(1.0), "status lingers");

        // The finishing run clears the in-flight flag.
        rs.set_results(&stats(&[], &[]));
        assert!(!rs.is_running(), "not running after results land");
    }
}
