use std::sync::Arc;
use std::time::Instant;

use crate::RamUsage;
use crate::execution::RunError;
use crate::execution::identity::ExecutionNodeId;
use crate::execution::outcome::{ExecutionOutcome, LogEntry};
use crate::execution::report::{RunPhase, RunProgress};

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

#[derive(Default, Debug)]
pub(crate) struct WorkerStatusPublisher {
    status: Arc<WorkerStatus>,
}

impl WorkerStatusPublisher {
    fn prepare(&mut self, activity: WorkerActivity, kind: WorkerStatusKind) -> &mut WorkerStatus {
        let update = Arc::make_mut(&mut self.status);
        update.activity = activity;
        update.kind = kind;
        update.nodes.clear();
        update.logs.clear();
        update.cache_ram = RamUsage::default();
        update
    }

    pub(crate) fn activity(&mut self, activity: WorkerActivity) -> Arc<WorkerStatus> {
        self.prepare(activity, WorkerStatusKind::Activity);
        Arc::clone(&self.status)
    }

    pub(crate) fn patch(&mut self) -> WorkerStatusPatch<'_> {
        let activity = self.status.activity;
        self.prepare(activity, WorkerStatusKind::Patch);
        WorkerStatusPatch {
            status: &mut self.status,
        }
    }

    pub(crate) fn completed(
        &mut self,
        activity: WorkerActivity,
        mut outcome: ExecutionOutcome,
    ) -> Arc<WorkerStatus> {
        let kind = WorkerStatusKind::Completed {
            elapsed_secs: outcome.elapsed_secs,
            executed_node_count: outcome.executed_nodes.len(),
            cancelled: outcome.cancelled,
        };
        let node_count = outcome.executed_nodes.len()
            + outcome.cached_nodes.len()
            + outcome.missing_inputs.len()
            + outcome.node_errors.len()
            + outcome.node_ram.len();
        let update = self.prepare(activity, kind);
        update.nodes.reserve(node_count);
        update
            .nodes
            .extend(outcome.executed_nodes.drain(..).map(|node| NodeStatus {
                e_node_id: node.e_node_id,
                status: Some(NodeExecutionStatus::Executed {
                    elapsed_secs: node.elapsed_secs,
                }),
                ram: None,
            }));
        update
            .nodes
            .extend(outcome.cached_nodes.drain(..).map(|e_node_id| NodeStatus {
                e_node_id,
                status: Some(NodeExecutionStatus::Cached),
                ram: None,
            }));
        update
            .nodes
            .extend(outcome.missing_inputs.drain(..).map(|port| NodeStatus {
                e_node_id: port.e_node_id,
                status: Some(NodeExecutionStatus::MissingInputs),
                ram: None,
            }));
        update.nodes.extend(
            outcome
                .node_errors
                .drain(..)
                .filter(|failure| !matches!(&failure.error, RunError::Cancelled { .. }))
                .map(|failure| NodeStatus {
                    e_node_id: failure.e_node_id,
                    status: Some(NodeExecutionStatus::Errored {
                        error: failure.error,
                    }),
                    ram: None,
                }),
        );
        update
            .nodes
            .extend(outcome.node_ram.drain(..).map(|node| NodeStatus {
                e_node_id: node.e_node_id,
                status: None,
                ram: Some(node.usage),
            }));
        update.logs.append(&mut outcome.logs);
        update.cache_ram = outcome.cache_ram;
        Arc::clone(&self.status)
    }
}

#[derive(Debug)]
pub(crate) struct WorkerStatusPatch<'a> {
    status: &'a mut Arc<WorkerStatus>,
}

impl WorkerStatusPatch<'_> {
    pub(crate) fn push(&mut self, progress: RunProgress) {
        let update = Arc::get_mut(self.status)
            .expect("status patch must remain unpublished while it is populated");
        debug_assert_eq!(update.kind, WorkerStatusKind::Patch);
        let status = match progress.phase {
            RunPhase::Started { at } => NodeExecutionStatus::Running { at },
            RunPhase::Finished { elapsed_secs } => NodeExecutionStatus::Executed { elapsed_secs },
        };
        update.nodes.push(NodeStatus {
            e_node_id: progress.e_node_id,
            status: Some(status),
            ram: None,
        });
    }

    pub(crate) fn finish(self) -> Arc<WorkerStatus> {
        debug_assert_eq!(self.status.kind, WorkerStatusKind::Patch);
        debug_assert!(!self.status.nodes.is_empty());
        Arc::clone(self.status)
    }
}
