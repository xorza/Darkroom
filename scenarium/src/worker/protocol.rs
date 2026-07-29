use std::sync::Arc;

use crate::worker::error::WorkerError;
use tokio::sync::oneshot;

use crate::execution::cache::disk_store::DiskStore;
use crate::execution::compile::artifact::CompiledGraph;
use crate::execution::seeds::RunSeeds;
use crate::graph::address::NodeId;
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
    Update { compiled: Arc<CompiledGraph> },
    Clear,
    EvictCache { nodes: Vec<NodeId> },
    SetDiskStore(DiskStore),
    Run { seeds: RunSeeds },
    StartEventLoop,
    StopEventLoop,
    Sync { reply: oneshot::Sender<()> },
}
