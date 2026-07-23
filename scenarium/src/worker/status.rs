use std::time::Instant;

use crate::RamUsage;
use crate::execution::RunError;
use crate::execution::identity::ExecutionNodeId;
use crate::execution::outcome::LogEntry;

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub enum WorkerActivity {
    #[default]
    Idle,
    Executing,
    EventLoop,
    ExecutingEventLoop,
}

impl WorkerActivity {
    pub fn is_executing(self) -> bool {
        matches!(
            self,
            WorkerActivity::Executing | WorkerActivity::ExecutingEventLoop
        )
    }

    pub fn event_loop_active(self) -> bool {
        matches!(
            self,
            WorkerActivity::EventLoop | WorkerActivity::ExecutingEventLoop
        )
    }
}

#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub enum WorkerStatusKind {
    #[default]
    Activity,
    Patch,
    Completed {
        elapsed_secs: f64,
        executed_node_count: usize,
        cancelled: bool,
    },
}

#[derive(Clone, Debug)]
pub enum NodeExecutionStatus {
    Running { at: Instant },
    Cached,
    Executed { elapsed_secs: f64 },
    MissingInputs,
    Errored { error: RunError },
}

#[derive(Clone, Debug)]
pub struct NodeStatus {
    pub e_node_id: ExecutionNodeId,
    pub status: Option<NodeExecutionStatus>,
    pub ram: Option<RamUsage>,
}

#[derive(Clone, Default, Debug)]
pub struct WorkerStatus {
    pub activity: WorkerActivity,
    pub kind: WorkerStatusKind,
    pub nodes: Vec<NodeStatus>,
    pub logs: Vec<LogEntry>,
    pub cache_ram: RamUsage,
}
