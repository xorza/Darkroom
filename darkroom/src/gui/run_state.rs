//! Last graph run's centralized runtime state: per-node execution outcomes
//! and logs, plus the latest value each preview node published. One
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
use scenarium::DynamicValue;
use scenarium::ExecutionNodeId;
use scenarium::LogLevel;
use scenarium::NodeExecutionStatus;
use scenarium::NodeId;
use scenarium::RamUsage;
use scenarium::RunError;
use scenarium::WorkerActivity;
use scenarium::WorkerStatus;
use scenarium::WorkerStatusKind;

use crate::gui::preview_store::PreviewStore;

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
    /// Input ports the last run could not satisfy, by index on this node — the run's
    /// own verdict, so a port bound to a disabled or missing producer counts too.
    /// Unlike every other field here this does *not* fold onto enclosing instances:
    /// a port index names a port on one node and means nothing on the instance
    /// around it.
    missing_inputs: Vec<usize>,
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
    pub(crate) previews: PreviewStore,
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

    /// Whether `id` holds work that recomputes every run, per the installed
    /// program. `None` on the same terms as [`Self::is_sink`].
    pub(crate) fn is_impure(&self, id: NodeId) -> Option<bool> {
        self.compiled.as_ref()?.is_impure(id)
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

    /// The input ports the last run reported unsatisfied on this node. Read into the
    /// scene each rebuild so only the ports that actually went unfed glow, rather than
    /// every required one on a node the run flagged.
    pub(crate) fn missing_inputs(&self, id: NodeId) -> &[usize] {
        self.nodes
            .get(&id)
            .map(|n| n.missing_inputs.as_slice())
            .unwrap_or(&[])
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
                // Progress only ever reports a node's lambda starting or finishing, so a
                // live patch never carries the planner's missing-input verdict; the ports
                // arrive with the completed snapshot.
                NodeExecutionStatus::MissingInputs { .. } => ExecStatus::MissingInputs,
                NodeExecutionStatus::Errored { error, .. } => {
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
            node.missing_inputs.clear();
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
                    NodeExecutionStatus::MissingInputs { ports } => {
                        self.record_missing_inputs(&compiled, node.e_node_id, ports);
                        ExecStatus::MissingInputs
                    }
                    NodeExecutionStatus::Errored { error, .. } => {
                        self.record_error(&compiled, node.e_node_id, error);
                        ExecStatus::Errored
                    }
                };
                self.record_status(&compiled, node.e_node_id, status);
            }
            if node.ram.total() > 0 {
                for node_id in attributed_nodes(&compiled, node.e_node_id) {
                    self.nodes.entry(node_id).or_default().ram += node.ram;
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
        self.drop_empty_nodes();
    }

    /// Drop every node entry the last update left with nothing to show. The
    /// one definition of "empty" — a node keeps its slot while it carries a
    /// status, a log line, or retained RAM — so the two callers can't disagree
    /// about what survives a fold.
    fn drop_empty_nodes(&mut self) {
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
        // A preview's value is a run result: with the cache behind it evicted,
        // what it shows can no longer be re-derived without another run.
        self.previews.entries.clear();
        // Every node's RAM was just zeroed, so what survives here is exactly
        // the nodes still carrying a status or a log — the run results the
        // eviction deliberately leaves standing.
        self.drop_empty_nodes();
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

    /// Record the ports one flattened node went unfed on — against that node alone, not
    /// its enclosing instances: a port index identifies a port on this node, and the same
    /// index on the instance around it names an unrelated port. Two occurrences of one
    /// authored node (a graph instantiated twice) union their unfed ports, since the
    /// authored view shows a port that failed in *some* instance.
    fn record_missing_inputs(
        &mut self,
        compiled: &CompiledGraph,
        e_node_id: ExecutionNodeId,
        ports: &[usize],
    ) {
        let Some(node_id) = attributed_nodes(compiled, e_node_id).next() else {
            return;
        };
        let slot = self.nodes.entry(node_id).or_default();
        for &port in ports {
            if !slot.missing_inputs.contains(&port) {
                slot.missing_inputs.push(port);
            }
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

    /// Store what preview nodes published this frame, against the authored
    /// nodes that published them.
    ///
    /// Nothing to resolve, unlike the old pinned push: a preview is
    /// entry-only, so its execution id attributes to exactly one authored node
    /// and that node is the widget. A value whose id belongs to an earlier
    /// compile is dropped — the node it named may not exist any more, and a
    /// preview only ever shows the current run's value anyway.
    ///
    /// Stores only. `Editor::frame` runs the store's one reconcile pass, which
    /// is what releases a value whose node is gone and uploads a full-resolution
    /// texture for an open viewer.
    pub(crate) fn ingest_previews(
        &mut self,
        ui: &Ui,
        published: Vec<(ExecutionNodeId, DynamicValue)>,
    ) {
        if published.is_empty() {
            return;
        }
        let Some(compiled) = self.compiled.clone() else {
            return;
        };
        for (e_node_id, value) in published {
            let Ok(mut attribution) = compiled.attribution(e_node_id) else {
                continue;
            };
            let Some(node_id) = attribution.next() else {
                continue;
            };
            self.previews.ingest_preview(ui, node_id, value);
        }
    }

    /// Drop everything visible from a failed run: no glow, logs, or values.
    pub(crate) fn clear(&mut self) {
        self.nodes.clear();
        self.previews.entries.clear();
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

    use scenarium::CompiledGraphBuilder;
    use scenarium::FuncId;
    use scenarium::{LogEntry, LogLevel, NodeStatus};

    use crate::gui::preview_store::StoredContent;

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
                ram: RamUsage::default(),
            })
            .collect::<Vec<_>>();
        nodes.extend(errored.iter().map(|&e_node_id| NodeStatus {
            e_node_id,
            status: Some(NodeExecutionStatus::Errored {
                elapsed_secs: None,
                error: RunError::Invoke {
                    func_id: FuncId::from_u128(0),
                    message: "test error".into(),
                },
            }),
            ram: RamUsage::default(),
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
                ram: RamUsage::default(),
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
        state
            .previews
            .entries
            .insert(evicted_node, StoredContent::Text("shown".into()));
        state
            .previews
            .entries
            .insert(remaining_node, StoredContent::Text("kept".into()));

        state.clear_cache_projections();

        assert_eq!(state.cache_ram, RamUsage::default());
        assert_eq!(state.ram(evicted_node), RamUsage::default());
        assert_eq!(state.ram(remaining_node), RamUsage::default());
        assert_eq!(state.status(evicted_node), ExecStatus::Cached);
        assert_eq!(state.nodes[&remaining_node].logs[0].message, "kept");
        assert!(
            state.previews.entries.is_empty(),
            "a preview's value is a run result too, and goes with the rest"
        );
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
                elapsed_secs: Some(1.5),
                error: RunError::Invoke {
                    func_id: FuncId::from_u128(0),
                    message: "no light frames provided".into(),
                },
            }),
            ram: RamUsage::default(),
        });

        rs.apply_worker_status(&s);

        assert_eq!(rs.status(interior), ExecStatus::Errored);
        assert_eq!(rs.errors(interior), ["no light frames provided"]);
        assert_eq!(rs.status(inst), ExecStatus::Errored);
        assert_eq!(rs.errors(inst), ["no light frames provided"]);
    }

    /// Unfed ports land on the node that owns them and nowhere else: the enclosing
    /// instance takes the `MissingInputs` status (its subtree failed) but none of the
    /// port indices, which would name unrelated ports on its own interface. Two
    /// occurrences of one authored node union their ports.
    #[test]
    fn missing_input_ports_attribute_to_the_owning_node_only() {
        let interior = nid(1);
        let (inst_a, inst_b) = (nid(10), nid(20));
        let (e_a, e_b) = (eid(101), eid(102));
        let mut rs = run_state([(e_a, vec![inst_a], interior), (e_b, vec![inst_b], interior)]);

        let mut s = completed_status(&[], &[]);
        s.nodes.push(NodeStatus {
            e_node_id: e_a,
            status: Some(NodeExecutionStatus::MissingInputs { ports: vec![0, 2] }),
            ram: RamUsage::default(),
        });
        s.nodes.push(NodeStatus {
            e_node_id: e_b,
            status: Some(NodeExecutionStatus::MissingInputs { ports: vec![2, 3] }),
            ram: RamUsage::default(),
        });
        rs.apply_worker_status(&s);

        assert_eq!(rs.status(interior), ExecStatus::MissingInputs);
        assert_eq!(
            rs.missing_inputs(interior),
            [0, 2, 3],
            "both occurrences' ports, unioned, port 2 once"
        );
        assert_eq!(rs.status(inst_a), ExecStatus::MissingInputs);
        assert!(
            rs.missing_inputs(inst_a).is_empty(),
            "an instance carries the verdict but not its interior's port indices"
        );
        assert!(rs.missing_inputs(inst_b).is_empty());

        // A run that satisfies them replaces the whole projection, ports included.
        rs.apply_worker_status(&completed_status(&[(e_a, 1.0), (e_b, 1.0)], &[]));
        assert_eq!(rs.status(interior), ExecStatus::Executed(2.0));
        assert!(rs.missing_inputs(interior).is_empty());
    }

    /// A failed node arrives as one row carrying both what it cost and why it failed —
    /// not as an `Executed` row plus an `Errored` row that a fold has to reconcile.
    #[test]
    fn a_failure_is_one_row_carrying_its_own_run_time() {
        let node = nid(1);
        let e_node_id = eid(1);
        let mut rs = run_state([(e_node_id, vec![], node)]);

        let mut s = completed_status(&[], &[]);
        s.nodes.push(NodeStatus {
            e_node_id,
            status: Some(NodeExecutionStatus::Errored {
                elapsed_secs: Some(2.5),
                error: RunError::Invoke {
                    func_id: FuncId::from_u128(0),
                    message: "boom".into(),
                },
            }),
            ram: RamUsage { cpu: 7, gpu: 0 },
        });
        rs.apply_worker_status(&s);

        assert_eq!(rs.status(node), ExecStatus::Errored);
        assert_eq!(rs.errors(node), ["boom"]);
        assert_eq!(
            rs.ram(node),
            RamUsage { cpu: 7, gpu: 0 },
            "the same row carries the RAM it kept"
        );
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
