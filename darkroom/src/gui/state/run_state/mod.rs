//! Last graph run's centralized runtime state: per-node execution outcomes
//! and logs, plus the latest value each preview node published. One
//! [`RunState`] per [`App`], updated as worker reports arrive.
//!
//! **Two halves with different owners.** `nodes` and `previews` belong to the open document and
//! die with it ([`RunState::clear`]). `compiled`, `activity` and `cache_ram` describe the
//! *worker* and are written only by what it reports: an in-flight run still reports against the
//! program the worker acknowledged. A document swap asks the worker to drop its program and
//! cache, and the `Cleared` acknowledgement resets them. Within the first half, `previews` is
//! swept per node once a frame and `nodes` only wholesale; see the `nodes` field for why.
//!
//! A run's node statuses are keyed by execution id, and every report resolves each through the
//! worker-confirmed [`CompiledGraph`] to exactly one authoring node; an id this install never
//! emitted is a protocol violation, not a node to skip. Logs attribute the same way.
//!
//! A status is assigned, never folded: a **completed** summary clears the previous run before
//! writing, while live **progress** overwrites one node in place. Progress is a liveness cue and
//! the completed summary is the authority that corrects it at the end.
//!
//! [`App`]: crate::gui::app::App

pub(crate) mod run_outcome;

use std::collections::{HashMap, VecDeque};
use std::mem;
use std::sync::Arc;
use std::time::Instant;

use palantir::Ui;

use crate::core::document::Document;
use crate::core::runtime_host::RuntimeHost;
use crate::core::status_log::{StatusFamily, StatusLog};
use crate::core::wake::Wake;
use common::ErrorChain;
use scenarium::CompiledGraph;
use scenarium::DynamicValue;
use scenarium::LogLevel;
use scenarium::NodeExecStatus;
use scenarium::NodeId;
use scenarium::RamUsage;
use scenarium::RunError;
use scenarium::WorkerActivity;
use scenarium::{RunPhase, RunSummary};
use scenarium::{WorkerError, WorkerReport};

use crate::gui::state::preview_store::PreviewStore;
use crate::gui::state::run_state::run_outcome::RunOutcome;

/// A report row can only name a node of the compiled graph the worker acknowledged, so a miss
/// is a protocol violation.
fn assert_reported(compiled: &CompiledGraph, node_id: NodeId) {
    debug_assert!(
        compiled.contains(node_id),
        "worker report identity must belong to the acknowledged compiled graph"
    );
}

/// Per-node execution outcome of the last run, with its detail, so the two cannot disagree.
#[derive(Clone, PartialEq, Default, Debug)]
pub(crate) enum ExecStatus {
    #[default]
    None,
    Cached,
    /// The node's wall-clock run time, in seconds.
    Executed(f64),
    /// The transient live state while the node computes: the instant it
    /// started, so the UI can show live elapsed-so-far.
    Running(Instant),
    /// The input ports the run could not satisfy, by index on the node, including a port bound
    /// to a disabled or missing producer.
    MissingInputs(Vec<usize>),
    /// The failure's message; `None` while only live progress has reported the failure, as its
    /// message arrives with the completed summary.
    Errored(Option<String>),
    /// Never ran: a node before it failed. Apart from `Errored` so the one
    /// node that failed reads as the cause and the ones it stopped do not.
    Skipped,
}

/// Everything the editor knows about one node from the last run.
#[derive(Default, Debug, PartialEq)]
pub(crate) struct NodeRunState {
    status: ExecStatus,
    logs: Vec<NodeLog>,
    /// RAM this node's cached output holds after the last run (system vs GPU).
    /// Zero unless the node retains a value; drives the node body's memory
    /// readout.
    ram: RamUsage,
}

impl NodeRunState {
    pub(crate) const fn status(&self) -> &ExecStatus {
        &self.status
    }

    pub(crate) const fn ram(&self) -> RamUsage {
        self.ram
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NodeLog {
    pub(crate) level: LogLevel,
    pub(crate) message: String,
}

/// Central runtime state for the current editor. Off the serialized state.
#[derive(Debug)]
pub(crate) struct RunState {
    /// What the last run said about each node — a **record of that run**, not a cache swept
    /// against the document, so deliberately absent from
    /// [`Document::holds_node`](crate::core::document::Document::holds_node)'s list of
    /// `NodeId`-keyed caches and from the once-a-frame pass that sweeps them. Reasons, in order
    /// of weight:
    ///
    /// 1. **Going stale on an edit is already what these mean.** A status says *what the last
    ///    run said*, never *what the graph would do now*. Sweeping on delete would make
    ///    delete-then-undo the one edit that discards results, only for the restored node: undo
    ///    re-attaches it under its original id, so its entry still reads.
    /// 2. **Nothing reads a dead entry.** Every reader is called from a node widget, which exists
    ///    only for a node the document holds.
    /// 3. **The memory is small and bounded.** An enum, a short `Vec<NodeLog>` and two strings
    ///    per node, reset by `replace_results` at the next completed run. The swept caches
    ///    release textures up to 8192² RGBA8, which earns them a per-frame pass.
    nodes: HashMap<NodeId, NodeRunState>,
    previews: PreviewStore,
    /// The program the worker acknowledged; every later progress or result belongs to it.
    compiled: Option<Arc<CompiledGraph>>,
    activity: WorkerActivity,
    /// The runs the worker reported ended, completed or failed: a count that only grows, so a
    /// reader that noted it before asking for a run knows when one ended after, however fast.
    /// The worker merges the run commands of one batch into one run, so this counts runs, not
    /// requests.
    runs_ended: u64,
    /// The reports a drain swaps in, kept for its buffer.
    reports: VecDeque<WorkerReport>,
    /// RAM held by the worker's runtime cache, as of the last report that measured it: a
    /// completed run, or an install, which is the only measurement a program with nothing to
    /// run ever produces. Eviction clears it until the next run, as eviction has no reply.
    cache_ram: RamUsage,
    /// The values preview nodes published since the last frame, kept for its capacity.
    published: HashMap<NodeId, DynamicValue>,
    /// What the last completed run came to; `None` before one, and after a
    /// [`Self::clear`].
    outcome: Option<RunOutcome>,
}

/// Where a run's reports come from, and where their verdicts land.
#[derive(Debug)]
pub(crate) struct RunFeed<'a> {
    pub(crate) runtime: &'a RuntimeHost,
    pub(crate) status: &'a mut StatusLog,
}

impl RunState {
    /// An empty projection, whose preview conversions wake the host through
    /// `wake` as each one finishes.
    pub(crate) fn new(wake: Wake) -> Self {
        Self {
            nodes: HashMap::new(),
            previews: PreviewStore::new(wake),
            compiled: None,
            activity: WorkerActivity::default(),
            runs_ended: 0,
            reports: VecDeque::new(),
            cache_ram: RamUsage::default(),
            published: HashMap::new(),
            outcome: None,
        }
    }

    /// What the last completed run came to, for the status bar.
    pub(crate) const fn outcome(&self) -> Option<RunOutcome> {
        self.outcome
    }

    /// The preview values the frame shows, and their conversions.
    pub(crate) const fn previews(&self) -> &PreviewStore {
        &self.previews
    }

    pub(crate) const fn previews_mut(&mut self) -> &mut PreviewStore {
        &mut self.previews
    }

    pub(crate) const fn activity(&self) -> WorkerActivity {
        self.activity
    }

    /// The runs the worker reported ended; see the field.
    pub(crate) const fn runs_ended(&self) -> u64 {
        self.runs_ended
    }

    /// RAM the worker's runtime cache holds, as of the last report that measured it.
    pub(crate) const fn cache_ram(&self) -> RamUsage {
        self.cache_ram
    }

    /// Fold the worker's reports, take in the values preview nodes published, and sweep the
    /// store against `document`.
    ///
    /// One entry point because the halves are ordered: the reports make `compiled` name the
    /// compile the worker just acknowledged, and a published value taken in first is gated
    /// against the *previous* compile and silently dropped.
    ///
    /// Runs from `App::sweep`, once a frame (every sweep is idempotent, while `record` runs
    /// *twice* on a frame carrying action input), and before the session rebuilds its scene.
    pub(crate) fn sync(&mut self, feed: RunFeed<'_>, ui: &Ui, document: &Document) {
        let RunFeed { runtime, status } = feed;
        self.drain_reports(runtime, status);
        self.sync_previews(runtime, ui, document);
    }

    /// Fold everything `runtime`'s worker has posted since the last frame.
    ///
    /// A completed run reprojects per-node `ExecStatus` and logs; a failed run clears both and
    /// surfaces in the status bar. Published values need a texture and are taken by
    /// [`Self::sync_previews`], so this names no `Ui`.
    fn drain_reports(&mut self, runtime: &RuntimeHost, status: &mut StatusLog) {
        let mut reports = mem::take(&mut self.reports);
        self.runs_ended += runtime.drain_worker(&mut reports);
        while let Some(report) = reports.pop_front() {
            self.apply_report(report, status);
        }
        self.reports = reports;
    }

    /// Fold one report; split out so a test can state the report stream.
    fn apply_report(&mut self, report: WorkerReport, status: &mut StatusLog) {
        match report {
            WorkerReport::Installed {
                compiled,
                cache_ram,
            } => {
                self.compiled = Some(compiled);
                // An install releases what the dropped nodes held; its figure is measured
                // afterwards and the only correction when the new program has nothing to run.
                self.cache_ram = cache_ram;
            }
            WorkerReport::Cleared => {
                self.compiled = None;
                self.cache_ram = RamUsage::default();
                self.clear();
            }
            WorkerReport::Error(error) => {
                if matches!(&error, WorkerError::Exec { .. }) {
                    self.runs_ended += 1;
                    self.clear();
                }
                // Logged as the worker posted it (`Arrival::log`).
                status.park_error(StatusFamily::Run, ErrorChain::new(&error).to_string());
            }
            WorkerReport::Activity(activity) => self.activity = activity,
            WorkerReport::Progress { node_id, phase } => self.apply_progress(node_id, phase),
            WorkerReport::Completed(summary) => {
                // A completed run supersedes a lingering failure message.
                status.succeeded(StatusFamily::Run);
                self.activity = summary.activity;
                self.runs_ended += 1;
                self.replace_results(&summary);
            }
        }
    }

    /// `id`'s last-run status, [`ExecStatus::None`] for a node the last run did not report.
    pub(crate) fn exec_status(&self, id: NodeId) -> &ExecStatus {
        self.node(id)
            .map_or(&ExecStatus::None, NodeRunState::status)
    }

    /// Everything the last run said about `id`, or `None` when it said
    /// nothing — one lookup for a reader that asks several things.
    pub(crate) fn node(&self, id: NodeId) -> Option<&NodeRunState> {
        self.nodes.get(&id)
    }

    pub(crate) fn logs(&self, id: NodeId) -> &[NodeLog] {
        self.nodes.get(&id).map_or(&[], |n| n.logs.as_slice())
    }

    /// Live progress: overwrite the node's status with the newest one, so `Running` is replaced
    /// by the finish that follows. A failure's message arrives with the completed summary.
    fn apply_progress(&mut self, node_id: NodeId, phase: RunPhase) {
        let compiled = self
            .compiled
            .as_ref()
            .expect("worker reported node progress before installing a compiled graph");
        assert_reported(compiled, node_id);
        self.nodes.entry(node_id).or_default().status = match phase {
            RunPhase::Started { at } => ExecStatus::Running(at),
            RunPhase::Succeeded { elapsed_secs } => ExecStatus::Executed(elapsed_secs),
            RunPhase::Failed { .. } => ExecStatus::Errored(None),
        };
    }

    /// Replace the last completed run with the worker's authoritative summary.
    fn replace_results(&mut self, update: &RunSummary) {
        let compiled = self
            .compiled
            .as_ref()
            .expect("worker reported results before installing a compiled graph");
        self.cache_ram = update.cache_ram;
        self.outcome = Some(RunOutcome::of(update));
        #[expect(
            clippy::iter_over_hash_type,
            reason = "each node is reset alone, so the order changes nothing"
        )]
        for node in self.nodes.values_mut() {
            node.status = ExecStatus::None;
            node.logs.clear();
            node.ram = RamUsage::default();
        }
        for node in &update.nodes {
            assert_reported(compiled, node.node_id);
            if let Some(status) = &node.status {
                let status = match status {
                    NodeExecStatus::Cached => ExecStatus::Cached,
                    NodeExecStatus::Executed { elapsed_secs } => {
                        ExecStatus::Executed(*elapsed_secs)
                    }
                    NodeExecStatus::MissingInputs { ports } => {
                        ExecStatus::MissingInputs(ports.clone())
                    }
                    NodeExecStatus::Errored {
                        error: RunError::SkippedUpstream,
                    } => ExecStatus::Skipped,
                    NodeExecStatus::Errored { error } => {
                        ExecStatus::Errored(Some(ErrorChain::new(error).to_string()))
                    }
                };
                self.nodes.entry(node.node_id).or_default().status = status;
            }
            if node.ram.total() > 0 {
                self.nodes.entry(node.node_id).or_default().ram = node.ram;
            }
        }
        for entry in &update.logs {
            assert_reported(compiled, entry.node_id);
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

    /// Project an eviction of `evicted` — the nodes
    /// [`RuntimeHost::evict_cache`] resolved it to, which is the seed's whole
    /// downstream cone, not just the node the user clicked.
    ///
    /// Successful eviction has no reply, so this is a projection, not a report: it drops what
    /// the worker is about to drop, and the next completed run replaces it with measured truth.
    ///
    /// The global total cannot be projected: it deduplicates values shared between nodes by
    /// pointer, so subtracting per-node figures would over-count. It reads as unknown (the status
    /// bar drops the clause) until the next run measures it.
    pub(crate) fn clear_cache_projections(&mut self, evicted: &[NodeId]) {
        self.cache_ram = RamUsage::default();
        for node_id in evicted {
            if let Some(node) = self.nodes.get_mut(node_id) {
                node.ram = RamUsage::default();
            }
            // With its cache evicted a preview cannot be re-derived without a run; one outside
            // the cone is still cached and stays on screen.
            self.previews.remove(*node_id);
        }
        // The evicted RAM is zeroed, so only nodes carrying a status or a log survive.
        self.drop_empty_nodes();
    }

    /// The preview store's per-frame upkeep: queue what nodes published, land the conversions
    /// that finished, then sweep what the document no longer holds.
    ///
    /// Ordered: a value published this frame queues before the landing, so a conversion of an
    /// older publish of the same node finds itself superseded, and both come before the sweep.
    ///
    /// A preview is entry-only, so its execution id attributes to exactly one authored node. A
    /// value whose id belongs to an earlier compile is dropped: its node may be gone, and a
    /// preview only shows the current run's value.
    fn sync_previews(&mut self, runtime: &RuntimeHost, ui: &Ui, document: &Document) {
        runtime.drain_previews(&mut self.published);
        #[expect(
            clippy::iter_over_hash_type,
            reason = "each preview lands under its own node, so the order changes nothing"
        )]
        for (node_id, value) in self.published.drain() {
            // The borrow ends with the condition, so no clone of the compile is needed.
            if self
                .compiled
                .as_ref()
                .is_some_and(|compiled| compiled.contains(node_id))
            {
                self.previews.publish(node_id, value);
            }
        }
        self.previews.land(ui);
        self.previews.reconcile(document);
    }

    /// Drop the document-derived half: no statuses, timings, logs, or published
    /// values. The worker-stream half is left alone — see the module doc.
    ///
    /// Two callers: a run that failed or was cleared, and a document swap
    /// (`App::adopt_document`), where the results describe a document no longer open.
    pub(crate) fn clear(&mut self) {
        self.nodes.clear();
        self.previews.clear();
        self.outcome = None;
    }
}

#[cfg(feature = "internals")]
pub(crate) mod internals {
    #[cfg(test)]
    use scenarium::{NodeId, RamUsage};

    use crate::gui::state::run_state::RunState;
    #[cfg(test)]
    use std::sync::Arc;

    #[cfg(test)]
    use scenarium::{CompiledGraph, RunSummary, WorkerReport};

    #[cfg(test)]
    use crate::core::status_log::StatusLog;
    #[cfg(test)]
    use crate::gui::state::run_state::{ExecStatus, NodeRunState};

    impl RunState {
        #[cfg(test)]
        pub(crate) fn status(&self, id: NodeId) -> ExecStatus {
            self.node(id)
                .map_or(ExecStatus::None, |node| node.status().clone())
        }

        #[cfg(test)]
        pub(crate) fn ram(&self, id: NodeId) -> RamUsage {
            self.node(id).map_or_default(NodeRunState::ram)
        }

        /// Take in `summary` as the worker's completed run of a program of exactly its rows'
        /// nodes, for a test of what a surface shows after a run.
        #[cfg(test)]
        pub(crate) fn complete(&mut self, summary: RunSummary) {
            let nodes = summary.nodes.iter().map(|row| row.node_id);
            self.compiled = Some(Arc::new(CompiledGraph::bare(nodes)));
            self.apply_report(
                WorkerReport::Completed(Arc::new(summary)),
                &mut StatusLog::default(),
            );
        }
    }
}

#[cfg(test)]
mod tests;
