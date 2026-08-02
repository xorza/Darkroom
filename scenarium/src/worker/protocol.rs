use std::sync::Arc;

use crate::worker::error::WorkerError;
use tokio::sync::oneshot;

use crate::execution::cache::disk_store::DiskStore;
use crate::execution::compile::compiled_graph::CompiledGraph;
use crate::execution::seeds::RunSeeds;
use crate::graph::identity::NodeId;
use crate::worker::status::WorkerStatus;

#[derive(Debug)]
pub enum WorkerReport {
    Installed(Arc<CompiledGraph>),
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
