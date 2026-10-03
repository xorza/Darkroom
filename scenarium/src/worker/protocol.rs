use std::sync::Arc;

use crate::worker::error::WorkerError;
use tokio::sync::oneshot;

use crate::RamUsage;
use crate::execution::cache::disk_store::DiskStore;
use crate::execution::compile::compiled_graph::CompiledGraph;
use crate::execution::report::RunPhase;
use crate::graph::identity::NodeId;
use crate::worker::activity::WorkerActivity;
use crate::worker::run_summary::RunSummary;

#[derive(Debug)]
pub enum WorkerReport {
    Installed {
        compiled: Arc<CompiledGraph>,
        /// What the runtime cache holds once its slots have been reconciled
        /// onto `compiled`. Carried here because an install is the other
        /// moment the cache changes size: a program that dropped nodes frees
        /// their values, and a host told only by completed runs would go on
        /// reporting the previous total until another run measured it.
        cache_ram: RamUsage,
    },
    /// The engine was emptied: no program, no cache. The cache footprint is
    /// zero by construction, so nothing is carried.
    Cleared,
    Error(WorkerError),
    /// What the worker is doing changed.
    Activity(WorkerActivity),
    /// A node's lambda started or finished, as the run reaches it.
    Progress {
        node_id: NodeId,
        phase: RunPhase,
    },
    /// A run completed. Reuses one allocation across runs while the host drops each summary
    /// before the next arrives.
    Completed(Arc<RunSummary>),
}

#[derive(Debug)]
pub enum WorkerMessage {
    Update {
        compiled: Arc<CompiledGraph>,
    },
    Clear,
    EvictCache {
        nodes: Vec<NodeId>,
    },
    /// Persist these nodes' resident disk-backed values now, rather than waiting
    /// for a run that recomputes them. Raised when a node's cache mode gains its
    /// disk bit while a value is already in RAM.
    FlushCache {
        nodes: Vec<NodeId>,
    },
    /// The same, over every installed node: what a store owes values computed
    /// while there was nowhere to persist them — an unsaved document that has
    /// just been given a root.
    ///
    /// Asked for rather than inferred from [`Self::SetDiskStore`]. A store is
    /// replaced for several unrelated reasons — a document opened, closed, or
    /// saved somewhere else — and only the host can tell which of them left
    /// values owing a blob. A
    /// worker guessing wrote the *outgoing* document's values into the
    /// *incoming* document's root, under ids nothing there would ever read.
    FlushAllCaches,
    /// Attach the store the cache persists to and serves from. Attaching alone
    /// writes nothing; see [`Self::FlushAllCaches`].
    SetDiskStore(DiskStore),
    /// Run every sink of the installed program.
    RunSinks,
    /// Run these nodes of the installed program and deliver every output —
    /// "run to this node". A disabled node named here runs for this run; one
    /// the program does not hold fails the run.
    RunNodes {
        nodes: Vec<NodeId>,
    },
    /// Run the subscribers of these events, as if the event loop fired them.
    #[cfg(test)]
    #[expect(
        clippy::absolute_paths,
        reason = "a test-only variant names its type in place of a cfg'd import"
    )]
    FireEvents {
        events: Vec<crate::graph::identity::EventPort>,
    },
    StartEventLoop,
    StopEventLoop,
    Sync {
        reply: oneshot::Sender<()>,
    },
}
