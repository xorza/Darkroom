use std::sync::Arc;

use crate::worker::error::WorkerError;
use tokio::sync::oneshot;

use crate::RamUsage;
use crate::execution::cache::disk_store::DiskStore;
use crate::execution::compile::compiled_graph::CompiledGraph;
use crate::execution::seeds::RunSeeds;
use crate::graph::identity::NodeId;
use crate::worker::status::WorkerStatus;

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
    Status(Arc<WorkerStatus>),
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
    SetDiskStore(DiskStore),
    Run {
        seeds: RunSeeds,
    },
    StartEventLoop,
    StopEventLoop,
    Sync {
        reply: oneshot::Sender<()>,
    },
}
