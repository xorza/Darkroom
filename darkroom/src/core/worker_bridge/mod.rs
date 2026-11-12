//! The bridge between the editor's `RuntimeHost` and the background
//! graph-evaluation `Worker` (`scenarium::worker`). The worker runs on its
//! own tokio runtime; this type owns that runtime, the worker handle, and the
//! [`ReportMailbox`] the worker's callback posts results into. The host loop
//! ([`App::record`](crate::gui::app::App)) drains it each frame and is woken
//! from off-thread via the [`Wake`] callback.

use std::collections::VecDeque;
use std::fmt;
use std::fmt::Debug;
use std::fmt::Formatter;
use std::sync::Arc;

use common::{BgRuntime, ErrorChain};
use scenarium::CompiledGraph;
use scenarium::DiskStore;
use scenarium::NodeId;
use scenarium::{Worker, WorkerExited, WorkerMessage, WorkerReport};

use crate::core::status_log;
use crate::core::wake::Wake;
use crate::core::worker_bridge::report_mailbox::ReportMailbox;

pub(crate) mod report_mailbox;

/// Every command here answers whether the worker took it. A send only fails once the worker task
/// is gone, and then *no report will ever arrive*, so a caller that assumed success would leave
/// the UI waiting on a run that can never report back.
pub(crate) struct WorkerBridge {
    worker: Worker,
    reports: Arc<ReportMailbox>,
    runtime: BgRuntime,
}

impl Debug for WorkerBridge {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("WorkerBridge").finish_non_exhaustive()
    }
}

impl WorkerBridge {
    /// Spin up the worker on a fresh multi-thread runtime. Starts memory-only; the host installs
    /// the disk-backed output cache via [`Self::set_disk_store`]. The callback runs on a worker
    /// thread: it posts the result into the mailbox and wakes the host, so the next frame drains
    /// it.
    pub(crate) fn new(wake: Wake) -> Self {
        let runtime = BgRuntime::new("darkroom-worker");
        let reports = Arc::new(ReportMailbox::default());
        let posted = Arc::clone(&reports);
        // `Worker`'s `tokio::spawn` needs an ambient runtime.
        let worker = runtime.enter(|| {
            Worker::new(move |report| {
                if let Some(arrival) = Arrival::of(&report) {
                    arrival.log();
                }
                posted.post(report);
                (wake)();
            })
        });
        Self {
            worker,
            reports,
            runtime,
        }
    }

    /// Install a compiled program. The worker acknowledges it with
    /// `WorkerReport::Installed` before processing reports from later commands.
    pub(crate) fn install(&self, compiled: Arc<CompiledGraph>) -> Result<(), WorkerExited> {
        self.worker.send(WorkerMessage::Update { compiled })
    }

    /// Drop the installed program and everything its cache holds. The worker
    /// acknowledges it with `WorkerReport::Cleared`.
    pub(crate) fn clear(&self) -> Result<(), WorkerExited> {
        self.worker.send(WorkerMessage::Clear)
    }

    /// Install the current program and evict an authored node's cache cone as one worker commit.
    /// Stopping the event loop keeps it from repopulating the entries being removed.
    pub(crate) fn install_and_evict_cache(
        &self,
        compiled: Arc<CompiledGraph>,
        node_id: NodeId,
    ) -> Result<(), WorkerExited> {
        self.worker.send_many([
            WorkerMessage::Update { compiled },
            WorkerMessage::EvictCache {
                node_ids: vec![node_id],
            },
            WorkerMessage::StopEventLoop,
        ])
    }

    /// Install the current program and persist an authored node's resident disk-backed value as
    /// one worker commit. The install has to lead: the engine reads the node's cache mode off the
    /// *installed* program, so a flush against the previous one would find the node not yet
    /// disk-backed and write nothing.
    ///
    /// No `StopEventLoop`, unlike the eviction: a flush leaves nothing for a running loop to
    /// repopulate.
    pub(crate) fn install_and_flush_cache(
        &self,
        compiled: Arc<CompiledGraph>,
        node_id: NodeId,
    ) -> Result<(), WorkerExited> {
        self.worker.send_many([
            WorkerMessage::Update { compiled },
            WorkerMessage::FlushCache {
                node_ids: vec![node_id],
            },
        ])
    }

    /// Execute every sink in the installed program.
    pub(crate) fn run_sinks(&self) -> Result<(), WorkerExited> {
        self.worker.send(WorkerMessage::RunSinks)
    }

    /// Execute these exact nodes in the installed program and deliver their outputs.
    pub(crate) fn run_nodes(
        &self,
        node_ids: impl IntoIterator<Item = NodeId>,
    ) -> Result<(), WorkerExited> {
        self.worker.send(WorkerMessage::RunNodes {
            node_ids: node_ids.into_iter().collect(),
        })
    }

    /// Point the engine's output cache at another store root, such as the active document's.
    /// Attaching writes nothing; see [`Self::flush_all_caches`].
    pub(crate) fn set_disk_store(&self, cache: DiskStore) -> Result<(), WorkerExited> {
        self.worker.send(WorkerMessage::SetDiskStore(cache))
    }

    /// Write every installed node's resident disk-backed value into the attached store, owed by
    /// values computed while there was nowhere to put them. The worker's apply order puts it
    /// after any `SetDiskStore` in the same batch.
    pub(crate) fn flush_all_caches(&self) -> Result<(), WorkerExited> {
        self.worker.send(WorkerMessage::FlushAllCaches)
    }

    /// Request cancellation of the in-flight run. Coarse: the running node finishes, but no
    /// further nodes are scheduled. A shared atomic the executor polls, with no channel
    /// round-trip.
    pub(crate) fn cancel_run(&self) {
        self.worker.request_cancel();
    }

    /// Start the installed program's event loop.
    pub(crate) fn start_event_loop(&self) -> Result<(), WorkerExited> {
        self.worker.send(WorkerMessage::StartEventLoop)
    }

    /// Stop the event loop (aborts the per-event tasks).
    pub(crate) fn stop_event_loop(&self) -> Result<(), WorkerExited> {
        self.worker.send(WorkerMessage::StopEventLoop)
    }

    /// Swap everything the worker posted since the last frame into `reports`, which is empty,
    /// merged ([`ReportMailbox`]); returns the count of runs that ended in a report a later one
    /// superseded.
    pub(crate) fn drain(&self, reports: &mut VecDeque<WorkerReport>) -> u64 {
        self.reports.drain(reports)
    }
}

impl Drop for WorkerBridge {
    fn drop(&mut self) {
        if let Err(error) = self.runtime.block_on(self.worker.exit()) {
            tracing::error!(
                error = %ErrorChain::new(&error),
                "worker task failed during shutdown"
            );
        }
    }
}

/// What a report records in the log, logged as the worker posts it. The log then holds each run in
/// the order and at the time it ran, also while a hidden window folds nothing and the mailbox
/// merges the report away; the fold only shows it.
#[derive(Debug, PartialEq, Eq)]
enum Arrival {
    /// A worker error, in the line the status log shows it with.
    Error(String),
    /// A run cancelled after `executed_node_count` nodes.
    Cancelled { executed_node_count: usize },
}

impl Arrival {
    fn of(report: &WorkerReport) -> Option<Self> {
        match report {
            WorkerReport::Error(error) => Some(Self::Error(ErrorChain::new(error).to_string())),
            WorkerReport::Completed(summary) if summary.cancelled => Some(Self::Cancelled {
                executed_node_count: summary.executed_node_count,
            }),
            WorkerReport::Installed { .. }
            | WorkerReport::Cleared
            | WorkerReport::Activity(_)
            | WorkerReport::Progress { .. }
            | WorkerReport::Completed(_) => None,
        }
    }

    fn log(&self) {
        match self {
            Self::Error(line) => status_log::log_error(line),
            Self::Cancelled {
                executed_node_count,
            } => tracing::info!("run cancelled after {executed_node_count} node(s)"),
        }
    }
}

#[cfg(test)]
mod tests;
