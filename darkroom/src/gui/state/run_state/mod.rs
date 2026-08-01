//! Last graph run's centralized runtime state: per-node execution outcomes
//! and logs, plus the latest value each preview node published. One
//! [`RunState`] per [`App`], updated as worker reports arrive.
//!
//! **Two halves with different owners.** `nodes` and `previews` belong to the
//! open document and die with it ([`RunState::clear`]); `compiled`, `activity`
//! and `cache_ram` describe the *worker* and outlive a document swap, because
//! an in-flight run still reports against the program the worker acknowledged
//! and its cache still holds what it holds.
//!
//! Within the first half the two differ again, and on purpose: `previews` is
//! swept per node once a frame, `nodes` only wholesale at a document swap or
//! the next completed run. See the field docs for why a run record is not a
//! node-keyed cache.
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
//! [`App`]: crate::gui::app::App

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use palantir::Ui;

use crate::core::runtime_host::RuntimeHost;
use scenarium::CompiledGraph;
use scenarium::DynamicValue;
use scenarium::LogLevel;
use scenarium::NodeExecutionStatus;
use scenarium::NodeId;
use scenarium::RamUsage;
use scenarium::RunError;
use scenarium::WorkerActivity;
use scenarium::WorkerStatus;
use scenarium::WorkerStatusKind;
use scenarium::{WorkerError, WorkerReport};

use crate::gui::state::preview_store::PreviewStore;

/// A report row can only name a node of the compiled graph the worker
/// acknowledged, so a miss is a protocol violation rather than a state to
/// tolerate.
fn assert_reported(compiled: &CompiledGraph, node_id: NodeId) {
    assert!(
        compiled.contains(node_id),
        "worker report identity must belong to the acknowledged compiled graph"
    );
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
    /// What the last run said about each node, keyed by the node that produced
    /// it — a **record of that run**, not a cache swept against the document.
    ///
    /// So it is deliberately absent from
    /// [`Document::holds_node`](crate::core::document::Document::holds_node)'s
    /// list of `NodeId`-keyed caches, and from the once-a-frame pass that
    /// sweeps them. Three reasons, in order of weight:
    ///
    /// 1. **Going stale on an edit is already what these mean.** Rewire a
    ///    node's input and its `Executed` glow persists untouched until the
    ///    next run — nothing on the edit path touches this. A status says
    ///    *what the last run said*, never *what the graph would do now*.
    ///    Sweeping on delete would make delete-then-undo the one edit that
    ///    discards results, and only for the restored node while every
    ///    neighbour from the same run keeps theirs: `UndoStep::RemoveNode`
    ///    re-attaches the node under its original id, so its entry is still
    ///    here and still reads.
    /// 2. **Nothing reads a dead entry.** Every reader below is called from a
    ///    node widget, which exists only for a node the document holds. An
    ///    entry outliving its node costs memory and nothing else.
    /// 3. **That memory is small and already bounded.** An enum, a short
    ///    `Vec<NodeLog>` and two strings per node, capped by the last run's
    ///    output — and `replace_results` resets every entry at the next
    ///    completed run. The caches that *are* swept release textures up to
    ///    8192² RGBA8, which is what earns them a per-frame pass.
    nodes: HashMap<NodeId, NodeRunState>,
    pub(crate) previews: PreviewStore,
    /// The program acknowledged by the worker's ordered report stream. Every
    /// subsequent progress/result payload belongs to this exact compile.
    pub(crate) compiled: Option<Arc<CompiledGraph>>,
    pub(crate) activity: WorkerActivity,
    /// RAM held by the worker's runtime cache after its latest run (system RAM
    /// vs GPU VRAM). Explicit eviction clears this projection until the next
    /// run because successful eviction is fire-and-forget.
    pub(crate) cache_ram: RamUsage,
}

impl RunState {
    /// Fold everything `runtime`'s worker has posted since the last frame into
    /// this projection.
    ///
    /// A completed run reprojects per-node `ExecStatus` (the status glow) and
    /// per-node logs (the inspector's Log section); a failed run clears both
    /// and surfaces in the status bar. A preview node's published value lands
    /// in the centralized preview store, which uploads its small thumbnail;
    /// visible cards and viewers read the new value during the frame already
    /// scheduled by the worker's wake callback.
    ///
    /// Pulls rather than being pushed to: the worker is headless
    /// ([`RuntimeHost`] names no GUI type), while this projection registers
    /// textures and so needs a `Ui`. Reading across the boundary in this
    /// direction keeps the frame context out of the runtime, and puts the fold
    /// beside the state it writes — every arm below lands on `self`, and only
    /// the status line reaches back.
    ///
    /// Called before the session rebuilds its scene, so the projections it
    /// reads reflect the latest run.
    pub(crate) fn drain_from(&mut self, runtime: &mut RuntimeHost, ui: &Ui) {
        // Owned, so the channel borrow is gone before the status writes
        // below — both come off the same `runtime`.
        let events = runtime.drain_worker();
        for report in events {
            match report {
                WorkerReport::Installed(compiled) => {
                    self.compiled = Some(compiled);
                }
                WorkerReport::Cleared => {
                    self.compiled = None;
                    self.clear();
                }
                WorkerReport::Error(error) => {
                    if matches!(&error, WorkerError::Execution { .. }) {
                        self.clear();
                    }
                    runtime.status.error(error.to_string());
                }
                WorkerReport::Status(status) => {
                    if let WorkerStatusKind::Completed {
                        executed_node_count,
                        cancelled,
                        ..
                    } = status.kind
                    {
                        if cancelled {
                            tracing::info!("run cancelled after {executed_node_count} node(s)");
                        }
                        // A completed run supersedes any lingering failure message
                        // from an earlier event-loop tick.
                        runtime.status.error = None;
                    }
                    self.apply_worker_status(&status);
                }
            }
        }
        // After the reports, so a value published during this run lands against
        // the compile the stream just acknowledged rather than the previous one.
        let previews = runtime.drain_previews();
        self.ingest_previews(ui, previews);
    }

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
                    self.record_error(&compiled, node.node_id, error);
                    ExecStatus::Errored
                }
            };
            assert_reported(&compiled, node.node_id);
            self.nodes.entry(node.node_id).or_default().status = status;
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
                        self.record_missing_inputs(&compiled, node.node_id, ports);
                        ExecStatus::MissingInputs
                    }
                    NodeExecutionStatus::Errored { error, .. } => {
                        self.record_error(&compiled, node.node_id, error);
                        ExecStatus::Errored
                    }
                };
                assert_reported(&compiled, node.node_id);
                self.nodes.entry(node.node_id).or_default().status = status;
            }
            if node.ram.total() > 0 {
                assert_reported(&compiled, node.node_id);
                self.nodes.entry(node.node_id).or_default().ram = node.ram;
            }
        }
        for entry in &update.logs {
            assert_reported(&compiled, entry.node_id);
            self.nodes
                .entry(entry.node_id)
                .or_default()
                .logs
                .push(NodeLog {
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
        node_id: NodeId,
        ports: &[usize],
    ) {
        assert_reported(compiled, node_id);
        let slot = self.nodes.entry(node_id).or_default();
        slot.missing_inputs.clear();
        slot.missing_inputs.extend_from_slice(ports);
    }

    /// Record one run error's message against the node that failed, so the
    /// inspector can show the actual cause instead of a bare "errored".
    fn record_error(&mut self, compiled: &CompiledGraph, node_id: NodeId, error: &RunError) {
        assert_reported(compiled, node_id);
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
    pub(crate) fn ingest_previews(&mut self, ui: &Ui, published: Vec<(NodeId, DynamicValue)>) {
        if published.is_empty() {
            return;
        }
        let Some(compiled) = self.compiled.clone() else {
            return;
        };
        for (node_id, value) in published {
            if !compiled.contains(node_id) {
                continue;
            }
            self.previews.ingest_preview(ui, node_id, value);
        }
    }

    /// Drop the document-derived half: no glow, timings, logs, or published
    /// values. The worker-stream half is left alone — see the module doc.
    ///
    /// Two callers, one meaning ("nothing on screen came from a run"): a run
    /// that failed or was cleared, and a document swap
    /// (`App::adopt_document`), where the results describe a document that is
    /// no longer open.
    pub(crate) fn clear(&mut self) {
        self.nodes.clear();
        self.previews.entries.clear();
    }
}

#[cfg(test)]
mod tests;
