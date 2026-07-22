use std::sync::Arc;

use tokio::sync::oneshot;

use crate::execution::compile::CompiledGraph;
use crate::execution::disk_store::DiskStore;
use crate::execution::identity::ExecutionEventPort;
use crate::execution::report::{PinnedOutputs, RunProgress};
use crate::execution::stats::ExecutionStats;
use crate::execution::{Result, RunSeeds};

#[derive(Debug)]
pub enum WorkerReport {
    Installed(Arc<CompiledGraph>),
    Cleared,
    Progress(RunProgress),
    PinnedOutputs(PinnedOutputs),
    Finished(Result<ExecutionStats>),
}

#[derive(Debug)]
pub enum WorkerMessage {
    Exit,
    InjectEvents { events: Vec<ExecutionEventPort> },
    Update { compiled: Arc<CompiledGraph> },
    Clear,
    SetDiskStore(DiskStore),
    Run { seeds: RunSeeds },
    StartEventLoop,
    StopEventLoop,
    Sync { reply: oneshot::Sender<()> },
}

#[derive(Debug, thiserror::Error)]
#[error("worker task has exited")]
pub struct WorkerExited;
