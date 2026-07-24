use std::sync::Arc;

use tokio::sync::oneshot;

use crate::execution::compile::CompiledGraph;
use crate::execution::disk_store::DiskStore;
use crate::execution::error::Error;
use crate::execution::report::PinnedOutputs;
use crate::execution::seeds::RunSeeds;
use crate::graph::NodeId;
use crate::worker::status::WorkerStatus;

#[derive(Debug, thiserror::Error)]
pub enum WorkerError {
    #[error("execution failed: {error}")]
    Execution {
        #[source]
        error: Error,
    },
    #[error("cache eviction failed for {failure_count} node(s): {details}")]
    CacheEviction {
        failure_count: usize,
        details: String,
    },
}

#[derive(Debug)]
pub enum WorkerReport {
    Installed(Arc<CompiledGraph>),
    Cleared,
    Error(WorkerError),
    Status(Arc<WorkerStatus>),
    PinnedOutputs(PinnedOutputs),
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

#[derive(Debug, thiserror::Error)]
#[error("worker task has exited")]
pub struct WorkerExited;
