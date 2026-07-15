use tokio::sync::oneshot;

use crate::execution::Result;
use crate::execution::compile::CompiledGraph;
use crate::execution::disk_store::DiskStore;
use crate::execution::event::EventRef;
use crate::execution::identity::NodeAddress;
use crate::execution::report::{PinnedOutputs, RunProgress};
use crate::execution::stats::ExecutionStats;

#[derive(Debug)]
pub enum WorkerReport {
    Progress(RunProgress),
    PinnedOutputs(PinnedOutputs),
    Finished(Result<ExecutionStats>),
}

#[derive(Debug)]
pub enum WorkerMessage {
    Exit,
    InjectEvents { events: Vec<EventRef> },
    Update { compiled: CompiledGraph },
    SaveCaches { compiled: CompiledGraph },
    Clear,
    SetDiskStore(DiskStore),
    ExecuteSinks,
    ExecuteNodes { nodes: Vec<NodeAddress> },
    StartEventLoop,
    StopEventLoop,
    Sync { reply: oneshot::Sender<()> },
}

#[derive(Debug, thiserror::Error)]
#[error("worker task has exited")]
pub struct WorkerExited;
