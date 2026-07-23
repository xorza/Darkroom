//! Last graph run's centralized runtime state: per-node execution outcomes
//! and logs, plus the latest worker-pushed values for pinned outputs. One
//! [`RunState`] per [`Editor`], updated as worker reports arrive.
//!
//! Execution dissolves graphs and remaps interior node ids, so a run's
//! raw node statuses are keyed by *flattened* ids. [`RunState::apply_worker_status`]
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
use scenarium::NodeExecutionStatus;
use scenarium::RamUsage;
use scenarium::RunError;
use scenarium::WorkerActivity;
use scenarium::WorkerStatus;
use scenarium::WorkerStatusKind;
use scenarium::PinnedOutputs;

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
/// transient live state while a node computes; it carries the instant the
/// node started so the UI can show live elapsed-so-far.
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
    pub(crate) activity: WorkerActivity,
    /// RAM held by the worker's runtime cache after its latest run (system RAM
    /// vs GPU VRAM). Explicit eviction clears this projection until the next
    /// run because successful eviction is fire-and-forget.
    pub(crate) cache_ram: RamUsage,
}

impl RunState {
    pub(crate) fn status(&self, id: NodeId) -> ExecStatus {
        self.nodes.get(&id).map(|n| n.status).unwrap_or_default()
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

    /// RAM this node's cached output currently holds (zero if it holds nothing).
    /// Read into the scene each rebuild to drive the node body's memory readout.
    pub(crate) fn ram(&self, id: NodeId) -> RamUsage {
        self.nodes.get(&id).map(|n| n.ram).unwrap_or_default()
    }

    pub(crate) fn apply_worker_status(&mut self, update: &WorkerStatus) {
        let entering_execution =
            !self.activity.is_executing() && update.activity.is_executing();
        self.activity = update.activity;
        if entering_execution {
            self.nodes
                .retain(|_, node| node.status != ExecStatus::None || !node.logs.is_empty());
        }

        match update.kind {
            WorkerStatusKind::Activity => {}
            WorkerStatusKind::Patch => self.apply_node_patch(update),
            WorkerStatusKind::Completed { .. } => self.replace_results(update),
        }
    }

    fn apply_node_patch(&mut self, update: &WorkerStatus) {
        let compiled = Arc::clone(
            self.compiled
                .as_ref()
                .expect("worker reported node status before installing a compiled graph"),
        );
        for node in &update.nodes {
            let Some(status) = &node.status else {
                continue;
            };
            let status = match status {
                NodeExecutionStatus::Running { at } => ExecStatus::Running(*at),
                NodeExecutionStatus::Cached => ExecStatus::Cached,
                NodeExecutionStatus::Executed { elapsed_secs } => {
                    ExecStatus::Executed(*elapsed_secs)
                }
                NodeExecutionStatus::MissingInputs => ExecStatus::MissingInputs,
                NodeExecutionStatus::Errored { error } => {
                    self.record_error(&compiled, node.e_node_id, error);
                    ExecStatus::Errored
                }
            };
            for node_id in attributed_nodes(&compiled, node.e_node_id) {
                self.nodes.entry(node_id).or_default().status = status;
            }
        }
    }

    /// Replace the last completed run with the worker's authoritative snapshot.
    fn replace_results(&mut self, update: &WorkerStatus) {
        let compiled = Arc::clone(
            self.compiled
                .as_ref()
                .expect("worker reported results before installing a compiled graph"),
        );
        self.cache_ram = update.cache_ram;
        for node in self.nodes.values_mut() {
            node.status = ExecStatus::None;
            node.logs.clear();
            node.errors.clear();
            node.ram = RamUsage::default();
        }
        for node in &update.nodes {
            if let Some(status) = &node.status {
                let status = match status {
                    NodeExecutionStatus::Running { .. } => {
                        panic!("completed worker status contains a running node")
                    }
                    NodeExecutionStatus::Cached => ExecStatus::Cached,
                    NodeExecutionStatus::Executed { elapsed_secs } => {
                        ExecStatus::Executed(*elapsed_secs)
                    }
                    NodeExecutionStatus::MissingInputs => ExecStatus::MissingInputs,
                    NodeExecutionStatus::Errored { error } => {
                        self.record_error(&compiled, node.e_node_id, error);
                        ExecStatus::Errored
                    }
                };
                self.record_status(&compiled, node.e_node_id, status);
            }
            if let Some(ram) = node.ram {
                for node_id in attributed_nodes(&compiled, node.e_node_id) {
                    self.nodes.entry(node_id).or_default().ram += ram;
                }
            }
        }
        for entry in &update.logs {
            for node_id in attributed_nodes(&compiled, entry.e_node_id) {
                self.nodes.entry(node_id).or_default().logs.push(NodeLog {
                    level: entry.level,
                    message: entry.message.clone(),
                });
            }
        }
        self.nodes
            .retain(|_, n| n.status != ExecStatus::None || !n.logs.is_empty() || n.ram.total() > 0);
    }

    /// Successful eviction has no reply, so its affected cache residency cannot
    /// be projected exactly until the next run reports fresh status and pins.
    pub(crate) fn clear_cache_projections(&mut self) {
        self.cache_ram = RamUsage::default();
        for node in self.nodes.values_mut() {
            node.ram = RamUsage::default();
        }
        self.pinned_outputs.entries.clear();
        self.nodes.retain(|_, node| {
            node.status != ExecStatus::None || !node.logs.is_empty() || node.ram.total() > 0
        });
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
        self.nodes.clear();
        self.pinned_outputs.entries.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use scenarium::CompiledGraphBuilder;
    use scenarium::FuncId;
    use scenarium::{DynamicValue, LogEntry, LogLevel, NodeStatus};
    use scenarium::{OutputPort, PinnedOutput, StaticValue};

    use crate::gui::pinned_output::StoredContent;

    fn nid(n: u128) -> NodeId {
        NodeId::from_u128(n)
    }

    fn eid(n: u128) -> ExecutionNodeId {
        ExecutionNodeId::from_u128(n)
    }

    fn completed_status(
        executed: &[(ExecutionNodeId, f64)],
        errored: &[ExecutionNodeId],
    ) -> WorkerStatus {
        let mut nodes = executed
            .iter()
            .map(|&(e_node_id, elapsed_secs)| NodeStatus {
                e_node_id,
                status: Some(NodeExecutionStatus::Executed { elapsed_secs }),
                ram: None,
            })
            .collect::<Vec<_>>();
        nodes.extend(
            errored
                .iter()
                .map(|&e_node_id| NodeStatus {
                    e_node_id,
                    status: Some(NodeExecutionStatus::Errored {
                        error: RunError::Invoke {
                            func_id: FuncId::from_u128(0),
                            message: "test error".into(),
                        },
                    }),
                    ram: None,
                }),
        );
        WorkerStatus {
            kind: WorkerStatusKind::Completed {
                elapsed_secs: 0.0,
                executed_node_count: executed.len(),
                cancelled: false,
            },
            nodes,
            ..WorkerStatus::default()
        }
    }

    fn node_patch(
        activity: WorkerActivity,
        e_node_id: ExecutionNodeId,
        status: NodeExecutionStatus,
    ) -> WorkerStatus {
        WorkerStatus {
            activity,
            kind: WorkerStatusKind::Patch,
            nodes: vec![NodeStatus {
                e_node_id,
                status: Some(status),
                ram: None,
            }],
            ..WorkerStatus::default()
        }
    }

    fn activity_status(activity: WorkerActivity) -> WorkerStatus {
        WorkerStatus {
            activity,
            ..WorkerStatus::default()
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
    fn clearing_cache_projections_drops_ram_and_pins_but_keeps_run_results() {
        let evicted_node = nid(1);
        let remaining_node = nid(2);
        let mut state = run_state([
            (eid(101), vec![], evicted_node),
            (eid(102), vec![], remaining_node),
        ]);
        state.nodes.entry(evicted_node).or_default().status = ExecStatus::Cached;
        state.nodes.entry(evicted_node).or_default().ram = RamUsage { cpu: 11, gpu: 0 };
        state
            .nodes
            .entry(remaining_node)
            .or_default()
            .logs
            .push(NodeLog {
                level: LogLevel::Info,
                message: "kept".into(),
            });
        state.nodes.entry(remaining_node).or_default().ram = RamUsage { cpu: 13, gpu: 0 };
        state.cache_ram = RamUsage { cpu: 24, gpu: 0 };
        let evicted_port = OutputPort::new(evicted_node, 0);
        let remaining_port = OutputPort::new(remaining_node, 0);
        state
            .pinned_outputs
            .entries
            .insert(evicted_port, StoredContent::Text("old".into()));
        state
            .pinned_outputs
            .entries
            .insert(remaining_port, StoredContent::Text("kept".into()));

        state.clear_cache_projections();

        assert_eq!(state.cache_ram, RamUsage::default());
        assert_eq!(state.ram(evicted_node), RamUsage::default());
        assert_eq!(state.ram(remaining_node), RamUsage::default());
        assert_eq!(state.status(evicted_node), ExecStatus::Cached);
        assert_eq!(state.nodes[&remaining_node].logs[0].message, "kept");
        assert!(!state.pinned_outputs.entries.contains_key(&evicted_port));
        assert!(!state.pinned_outputs.entries.contains_key(&remaining_port));
    }

    #[test]
    fn node_patch_marks_all_attributed_nodes_running_then_executed() {
        let (interior, instance) = (nid(1), nid(2));
        let e_node_id = eid(100);
        let mut rs = run_state([(e_node_id, vec![instance], interior)]);

        rs.apply_worker_status(&node_patch(
            WorkerActivity::Executing,
            e_node_id,
            NodeExecutionStatus::Running { at: Instant::now() },
        ));
        assert!(matches!(rs.status(interior), ExecStatus::Running(_)));
        assert!(matches!(rs.status(instance), ExecStatus::Running(_)));

        rs.apply_worker_status(&node_patch(
            WorkerActivity::Executing,
            e_node_id,
            NodeExecutionStatus::Executed { elapsed_secs: 0.5 },
        ));
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
        rs.apply_worker_status(&completed_status(&[(e_node_id_a, 2.0), (e_node_id_b, 3.0)], &[]));

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
        rs.apply_worker_status(&completed_status(&[(e_node_id, 4.0)], &[]));

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
        rs.apply_worker_status(&completed_status(&[(e_node_id, 1.0)], &[e_node_id]));
        assert_eq!(rs.status(interior), ExecStatus::Errored);
        // The failure message rides along with the status — the inspector
        // shows it instead of a bare "errored".
        assert_eq!(rs.errors(interior), ["test error"]);
    }

    #[test]
    fn error_messages_attribute_to_instance() {
        let interior = nid(1);
        let inst = nid(10);
        let fail_e_node_id = eid(100);
        let mut rs = run_state([(fail_e_node_id, vec![inst], interior)]);

        let mut s = completed_status(&[], &[]);
        s.nodes.push(NodeStatus {
            e_node_id: fail_e_node_id,
            status: Some(NodeExecutionStatus::Errored {
                error: RunError::Invoke {
                    func_id: FuncId::from_u128(0),
                    message: "no light frames provided".into(),
                },
            }),
            ram: None,
        });

        rs.apply_worker_status(&s);

        assert_eq!(rs.status(interior), ExecStatus::Errored);
        assert_eq!(rs.errors(interior), ["no light frames provided"]);
        assert_eq!(rs.status(inst), ExecStatus::Errored);
        assert_eq!(rs.errors(inst), ["no light frames provided"]);
    }

    /// A log line emitted inside a graph instance attributes to both
    /// the interior node and the enclosing instance, preserving level +
    /// message in each editor node's own state.
    #[test]
    fn apply_worker_status_attributes_logs_to_interior_and_instance() {
        let interior = nid(1);
        let inst = nid(10);
        let e_node_id = eid(100);
        let mut rs = run_state([(e_node_id, vec![inst], interior)]);

        let mut s = completed_status(&[], &[]);
        s.logs.push(LogEntry {
            e_node_id,
            level: LogLevel::Warn,
            message: "hi".into(),
        });

        rs.apply_worker_status(&s);

        let i = rs.logs(interior);
        assert_eq!(i.len(), 1, "interior carries the line");
        assert_eq!(i[0].message, "hi");
        assert_eq!(i[0].level, LogLevel::Warn);
        let n = rs.logs(inst);
        assert_eq!(n.len(), 1);
        assert_eq!(n[0], i[0], "both attributed nodes carry the same line");
    }

    #[test]
    fn worker_activity_is_absolute_and_execution_keeps_results_until_completion() {
        let node = nid(1);
        let e_node_id = eid(1);
        let mut rs = run_state([(e_node_id, vec![], node)]);
        rs.apply_worker_status(&completed_status(&[(e_node_id, 1.0)], &[]));

        assert_eq!(rs.activity, WorkerActivity::Idle);
        rs.apply_worker_status(&activity_status(WorkerActivity::Executing));

        assert_eq!(rs.activity, WorkerActivity::Executing);
        assert_eq!(rs.status(node), ExecStatus::Executed(1.0), "status lingers");

        let mut completed = completed_status(&[], &[]);
        completed.activity = WorkerActivity::EventLoop;
        rs.apply_worker_status(&completed);
        assert_eq!(rs.activity, WorkerActivity::EventLoop);
        assert_eq!(rs.status(node), ExecStatus::None);

        rs.clear();
        assert!(
            rs.activity.event_loop_active(),
            "clearing projections preserves worker activity"
        );
        for activity in [
            WorkerActivity::Idle,
            WorkerActivity::EventLoop,
            WorkerActivity::ExecutingEventLoop,
            WorkerActivity::Executing,
            WorkerActivity::Idle,
        ] {
            rs.apply_worker_status(&activity_status(activity));
            assert_eq!(rs.activity, activity);
        }
    }
}
