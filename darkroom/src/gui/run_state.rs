//! Last graph run's centralized runtime state: per-node execution outcomes
//! and logs, plus the latest value each preview node published. One
//! [`RunState`] per [`Editor`], updated as worker reports arrive.
//!
//! A run's node statuses are keyed by execution id.
//! [`RunState::apply_worker_status`] resolves each through the
//! worker-confirmed [`CompiledGraph`] to the authoring node it came from —
//! exactly one, and a report naming an id this install never emitted is a
//! protocol violation rather than a node to skip. Logs attribute the same way.
//!
//! One execution node per authored node and one report row per execution node,
//! so a status is *assigned*, never folded: every write here is the last word
//! on that node for that report. The two report kinds still differ in scope —
//! the **completed** snapshot clears the previous run before writing, while
//! live **patches** overwrite in place, so a node's displayed status is
//! whichever report most recently mentioned it. Progress is a liveness cue and
//! the completed snapshot is the authority that corrects it at the end.
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

/// The authoring node one report row is about.
///
/// A report can only name an execution node of the compiled graph the worker
/// acknowledged, so a miss is a protocol violation rather than a state to
/// tolerate.
fn attributed_node(compiled: &CompiledGraph, e_node_id: ExecutionNodeId) -> NodeId {
    compiled
        .attribution(e_node_id)
        .expect("worker report identity must belong to the acknowledged compiled graph")
}

/// Per-node execution outcome of the last run. `Executed` carries the node's
/// wall-clock run time (seconds). `Running` is the transient live state while a
/// node computes; it carries the instant the node started so the UI can show
/// live elapsed-so-far.
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

/// Everything the editor knows about one node from the last run.
#[derive(Default, Debug)]
struct NodeRunState {
    status: ExecStatus,
    logs: Vec<NodeLog>,
    /// Human-readable message for this run's failure. `None` unless the node
    /// errored; drives the inspector's error detail so a failed node reads e.g.
    /// "no light frames provided", not just "errored".
    error: Option<String>,
    /// RAM this node's cached output holds after the last run (system vs GPU).
    /// Zero unless the node retains a value; drives the node body's memory
    /// readout.
    ram: RamUsage,
    /// Input ports the last run could not satisfy, by index on this node — the run's
    /// own verdict, so a port bound to a disabled or missing producer counts too.
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

    pub(crate) fn logs(&self, id: NodeId) -> &[NodeLog] {
        self.nodes
            .get(&id)
            .map(|n| n.logs.as_slice())
            .unwrap_or(&[])
    }

    /// This run's failure message for a node. `None` unless it errored.
    pub(crate) fn error(&self, id: NodeId) -> Option<&str> {
        self.nodes.get(&id)?.error.as_deref()
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

    /// Live progress: assign each row's status to its authored node as it
    /// arrives, overwriting whatever the last report said — a node's newest
    /// status is its status, and `Running` has to be replaceable by the
    /// `Executed` that follows it.
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
            let node_id = attributed_node(&compiled, node.e_node_id);
            self.nodes.entry(node_id).or_default().status = status;
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
            node.error = None;
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
                let node_id = attributed_node(&compiled, node.e_node_id);
                self.nodes.entry(node_id).or_default().status = status;
            }
            if node.ram.total() > 0 {
                let node_id = attributed_node(&compiled, node.e_node_id);
                self.nodes.entry(node_id).or_default().ram = node.ram;
            }
        }
        for entry in &update.logs {
            let node_id = attributed_node(&compiled, entry.e_node_id);
            self.nodes.entry(node_id).or_default().logs.push(NodeLog {
                level: entry.level,
                message: entry.message.clone(),
            });
        }
        self.drop_empty_nodes();
    }

    /// Drop every node entry the last update left with nothing to show. The
    /// one definition of "empty" — a node keeps its slot while it carries a
    /// status, a log line, or retained RAM — so the two callers can't disagree
    /// about what survives.
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

    /// Record the ports one node went unfed on. The run's own verdict is the
    /// whole list, so it replaces rather than unions.
    fn record_missing_inputs(
        &mut self,
        compiled: &CompiledGraph,
        e_node_id: ExecutionNodeId,
        ports: &[usize],
    ) {
        let node_id = attributed_node(compiled, e_node_id);
        let slot = self.nodes.entry(node_id).or_default();
        slot.missing_inputs.clear();
        slot.missing_inputs.extend_from_slice(ports);
    }

    /// Record one run error's message against the node that failed, so the
    /// inspector can show the actual cause instead of a bare "errored".
    fn record_error(
        &mut self,
        compiled: &CompiledGraph,
        e_node_id: ExecutionNodeId,
        error: &RunError,
    ) {
        let node_id = attributed_node(compiled, e_node_id);
        self.nodes.entry(node_id).or_default().error = Some(error.to_string());
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
            let Ok(node_id) = compiled.attribution(e_node_id) else {
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
mod tests {
    use super::*;

    use scenarium::CompiledGraphBuilder;
    use scenarium::FuncId;
    use scenarium::{LogLevel, NodeStatus};

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

    fn run_state(nodes: impl IntoIterator<Item = NodeId>) -> RunState {
        let mut builder = CompiledGraphBuilder::new();
        for node_id in nodes {
            builder.insert_node(node_id);
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
        let mut state = run_state([evicted_node, remaining_node]);
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
    fn node_patch_marks_the_attributed_node_running_then_executed() {
        let node = nid(1);
        let e_node_id = eid(1);
        let mut rs = run_state([node]);

        rs.apply_worker_status(&node_patch(
            WorkerActivity::Executing,
            e_node_id,
            NodeExecutionStatus::Running { at: Instant::now() },
        ));
        assert!(matches!(rs.status(node), ExecStatus::Running(_)));

        rs.apply_worker_status(&node_patch(
            WorkerActivity::Executing,
            e_node_id,
            NodeExecutionStatus::Executed { elapsed_secs: 0.5 },
        ));
        assert_eq!(rs.status(node), ExecStatus::Executed(0.5));

        // A node no event mentioned stays None.
        assert_eq!(rs.status(nid(99)), ExecStatus::None);
    }

    /// The completed snapshot is the authority: it clears the previous run
    /// before writing, so a node the new run says nothing about loses the
    /// status the old one gave it, and one it does report carries the failure
    /// message alongside the status.
    #[test]
    fn completed_snapshot_replaces_the_previous_run() {
        let executed = nid(1);
        let errored = nid(2);
        let mut rs = run_state([executed, errored]);

        rs.apply_worker_status(&completed_status(&[(eid(1), 1.0), (eid(2), 0.25)], &[]));
        assert_eq!(rs.status(executed), ExecStatus::Executed(1.0));
        assert_eq!(rs.status(errored), ExecStatus::Executed(0.25));
        assert_eq!(rs.error(errored), None);

        // Second run: only `errored` is reported, and it failed.
        rs.apply_worker_status(&completed_status(&[], &[eid(2)]));
        assert_eq!(rs.status(errored), ExecStatus::Errored);
        // The failure message rides along with the status — the inspector
        // shows it instead of a bare "errored".
        assert_eq!(rs.error(errored), Some("test error"));
        assert_eq!(
            rs.status(executed),
            ExecStatus::None,
            "the previous run's verdict does not survive a snapshot that omits the node"
        );
    }
}
