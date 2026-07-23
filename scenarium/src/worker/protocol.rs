use std::sync::Arc;

use tokio::sync::oneshot;

use crate::execution::compile::CompiledGraph;
use crate::execution::disk_store::DiskStore;
use crate::execution::identity::ExecutionEventPort;
use crate::execution::report::{PinnedOutputs, RunProgress};
use crate::execution::stats::ExecutionStats;
use crate::execution::{Error, RunSeeds};
use crate::graph::NodeId;

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkerLifecycle {
    ExecutionStarted,
    ExecutionStopped,
    EventLoopStarted,
    EventLoopStopped,
}

#[derive(Debug)]
pub enum WorkerReport {
    Installed(Arc<CompiledGraph>),
    Cleared,
    Error(WorkerError),
    Lifecycle(WorkerLifecycle),
    Progress(RunProgress),
    PinnedOutputs(PinnedOutputs),
    Finished(ExecutionStats),
}

#[derive(Debug)]
pub enum WorkerMessage {
    Exit,
    InjectEvents { events: Vec<ExecutionEventPort> },
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
