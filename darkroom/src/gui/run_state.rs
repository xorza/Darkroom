//! Last graph run's centralized runtime state: per-node execution outcomes
//! and logs, plus the latest worker-pushed values for pinned outputs. One
//! [`RunState`] per [`Editor`], updated as worker reports arrive.
//!
//! Execution dissolves graphs and remaps interior node ids, so a run's
//! raw node statuses are keyed by *flattened* ids.
//! [`RunState::apply_worker_status`] projects them through the
//! worker-confirmed [`CompiledGraph`] onto the authoring nodes: onto the
//! node itself and onto every ancestor composite instance, so an instance
//! node reflects its whole subtree. Logs attribute the same way.
//!
//! The two report kinds fold differently, on purpose:
//!
//! - The **completed** snapshot aggregates. It clears the previous run
//!   first, then merges every occurrence by severity (and sums `Executed`
//!   times), so an authored interior node reflects the worst outcome across
//!   its instances and an instance node reflects its whole subtree.
//! - Live **patches** show the latest status received per authored node,
//!   with no accumulation. An authored node that runs once per instance
//!   therefore reports whichever occurrence most recently changed state —
//!   including a later `Running` replacing an earlier `Errored`. Progress
//!   is a liveness cue, not a verdict; the completed snapshot is the
//!   authority and corrects it at the end of the run.
//!
//! [`Editor`]: crate::gui::app::editor::Editor

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use palantir::Ui;
use scenarium::CompiledGraph;
use scenarium::ExecutionNodeId;
use scenarium::LogLevel;
use scenarium::NodeExecutionStatus;
use scenarium::NodeId;
use scenarium::OutputPort;
use scenarium::PinnedOutputs;
use scenarium::RamUsage;
use scenarium::RunError;
use scenarium::WorkerActivity;
use scenarium::WorkerStatus;
use scenarium::WorkerStatusKind;

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
struct NodeRunState {
    status: ExecStatus,
    logs: Vec<NodeLog>,
    /// Human-readable messages for this run's failures, folded on the same
    /// attribution as `status` (a graph instance collects its subtree's).
    /// Empty unless the node errored; drives the inspector's error detail so
    /// a failed node reads e.g. "no light frames provided", not just "errored".
    errors: Vec<String>,
    /// RAM this node's cached output holds after the last run (system vs GPU),
    /// summed across its flattened contributors — a graph instance aggregates
    /// its interior. Zero unless the node retains a value; drives the node body's
    /// memory readout.
    ram: RamUsage,
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

    /// Whether `id` performs sink work, per the installed program. `None`
    /// before the first compile, or when the node covers no compiled work —
    /// see [`CompiledGraph::is_sink`].
    pub(crate) fn is_sink(&self, id: NodeId) -> Option<bool> {
        self.compiled.as_ref()?.is_sink(id)
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
        self.activity = update.activity;
        match update.kind {
            WorkerStatusKind::Activity => {}
            WorkerStatusKind::Patch => self.apply_node_patch(update),
            WorkerStatusKind::Completed { .. } => self.replace_results(update),
        }
    }

    /// Live progress: assign each occurrence's status to its authored nodes
    /// as it arrives. Deliberately last-write-wins rather than merged — see
    /// the module docs. Merging would also pin a node at `Running` forever,
    /// since `Executed` ranks *below* it and could never replace it.
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

    /// Route one flat node's delivered values to the authored ports they fill.
    ///
    /// A value arrives addressed to the node that *computed* it, which is the
    /// node the user pinned only for a leaf in the entry graph. Two things can
    /// claim it:
    ///
    /// - **Something pinned it.** `pinned_ports` resolves that through the
    ///   compiled program, so a graph instance's port finds the interior slot
    ///   backing it. Ports backed by several occurrences resolve to nothing —
    ///   one preview widget cannot show a value per instance.
    /// - **Nothing did, and it came off a top-level node.** A run root
    ///   delivers every output it has, pinned or not; that is what fills a
    ///   viewer tab whose pin has since been removed. An interior node's
    ///   unrequested outputs have no addressable port and are dropped.
    pub(crate) fn ingest_pinned_outputs(
        &mut self,
        ui: &Ui,
        pushed: PinnedOutputs,
        document: &Document,
    ) {
        let compiled = Arc::clone(
            self.compiled
                .as_ref()
                .expect("worker pushed outputs before installing a compiled graph"),
        );
        let mut attribution = attributed_nodes(&compiled, pushed.e_node_id);
        let leaf = attribution
            .next()
            .expect("execution attribution must start with its authored leaf");
        let top_level = attribution.next().is_none();

        let mut values = Vec::with_capacity(pushed.values.len());
        for output in pushed.values {
            let requested = compiled.pinned_ports(pushed.e_node_id, output.port_idx);
            let Some((last, rest)) = requested.split_last() else {
                if top_level {
                    values.push((OutputPort::new(leaf, output.port_idx), output.value));
                }
                continue;
            };
            // One slot answering two authored ports is an instance port
            // pinned over its own pinned interior producer — rare, so the
            // clones it costs are too.
            for port in rest {
                values.push((*port, output.value.clone()));
            }
            values.push((*last, output.value));
        }
        self.pinned_outputs.ingest(ui, values, document);
    }

    /// Drop everything visible from a failed run: no glow, logs, or pinned
    /// values.
    pub(crate) fn clear(&mut self) {
        self.nodes.clear();
        self.pinned_outputs.entries.clear();
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use std::sync::Arc;

    use scenarium::CompiledGraph;

    use crate::gui::run_state::RunState;

    /// A run state holding nothing but an installed program — what the scene
    /// needs to fold compiled facts (`is_sink`) onto its nodes.
    pub(crate) fn with_compiled(compiled: Arc<CompiledGraph>) -> RunState {
        RunState {
            compiled: Some(compiled),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use palantir::internals::UiHarness;

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
        nodes.extend(errored.iter().map(|&e_node_id| NodeStatus {
            e_node_id,
            status: Some(NodeExecutionStatus::Errored {
                error: RunError::Invoke {
                    func_id: FuncId::from_u128(0),
                    message: "test error".into(),
                },
            }),
            ram: None,
        }));
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
        rs.apply_worker_status(&completed_status(
            &[(e_node_id_a, 2.0), (e_node_id_b, 3.0)],
            &[],
        ));

        // Shared interior view: both instances' times sum (2 + 3).
        assert_eq!(rs.status(interior), ExecStatus::Executed(5.0));
        // Each instance node carries only its own run.
        assert_eq!(rs.status(inst_a), ExecStatus::Executed(2.0));
        assert_eq!(rs.status(inst_b), ExecStatus::Executed(3.0));
    }

    #[test]
    fn a_pinned_instance_port_is_filled_by_the_interior_slot_backing_it() {
        // A graph instance dissolves at compile time, so the value for its
        // port arrives addressed to an interior node the user never pinned.
        // The compiled program says which authored port that slot answers.
        let interior = nid(1);
        let instance = nid(10);
        let backing = eid(101);
        let instance_port = OutputPort::new(instance, 0);

        let mut builder = CompiledGraphBuilder::new();
        builder.insert_leaf(backing, [instance], interior);
        builder.insert_pinned_port(backing, 0, instance_port);
        let mut run_state = RunState {
            compiled: Some(builder.build()),
            ..Default::default()
        };

        let mut document = Document::default();
        document.graph.set_output_pinned(instance_port, true);
        let mut arena = UiHarness::arena();
        run_state.ingest_pinned_outputs(
            arena.ui(),
            PinnedOutputs {
                e_node_id: backing,
                values: vec![PinnedOutput {
                    port_idx: 0,
                    value: DynamicValue::Static(StaticValue::Int(7)),
                }],
            },
            &document,
        );

        assert!(matches!(
            &run_state.pinned_outputs.entries[&instance_port],
            StoredContent::Text(text) if text == "7"
        ));
        assert!(
            !run_state
                .pinned_outputs
                .entries
                .contains_key(&OutputPort::new(interior, 0)),
            "the value lands on the port that was pinned, not on the node that computed it"
        );
    }

    #[test]
    fn unrequested_outputs_fill_only_a_top_level_nodes_own_ports() {
        // A run root delivers every output it has, pinned or not. Those are
        // addressable only when the node is in the entry graph — an interior
        // node's port names one occurrence among possibly many, and the
        // preview widget cannot say which.
        let interior = nid(1);
        let (instance_a, instance_b) = (nid(10), nid(20));
        let (nested_a, nested_b) = (eid(101), eid(102));
        let top_level = nid(2);
        let top_level_occurrence = eid(103);
        let mut run_state = run_state([
            (nested_a, vec![instance_a], interior),
            (nested_b, vec![instance_b], interior),
            (top_level_occurrence, vec![], top_level),
        ]);
        let mut document = Document::default();
        let nested_port = OutputPort::new(interior, 0);
        let top_level_port = OutputPort::new(top_level, 0);
        document.graph.set_output_pinned(nested_port, true);
        document.graph.set_output_pinned(top_level_port, true);
        let mut arena = UiHarness::arena();

        let mut push = |run_state: &mut RunState, e_node_id, value| {
            run_state.ingest_pinned_outputs(
                arena.ui(),
                PinnedOutputs {
                    e_node_id,
                    values: vec![PinnedOutput {
                        port_idx: 0,
                        value: DynamicValue::Static(StaticValue::Int(value)),
                    }],
                },
                &document,
            );
        };

        push(&mut run_state, nested_a, 7);
        push(&mut run_state, nested_b, 8);
        assert!(
            !run_state.pinned_outputs.entries.contains_key(&nested_port),
            "neither occurrence claims the shared port"
        );

        // The entry graph has exactly one occurrence per node, so an
        // unrequested output there names one port and fills it.
        push(&mut run_state, top_level_occurrence, 9);
        assert!(matches!(
            &run_state.pinned_outputs.entries[&top_level_port],
            StoredContent::Text(text) if text == "9"
        ));
        assert_eq!(run_state.pinned_outputs.entries.len(), 1);
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
