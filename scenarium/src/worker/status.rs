use std::sync::Arc;

use crate::RamUsage;
use crate::execution::outcome::LogEntry;
use crate::execution::outcome::{ExecutionOutcome, NodeExecutionStatus, NodeStatus};
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
    /// Claim the retained allocation for the next snapshot. When the previously published
    /// one is still queued at the consumer the allocation can't be reused — and
    /// `Arc::make_mut` would deep-clone (per-element `String`s and all) vectors this
    /// function clears three lines later — so publish into a fresh one instead and let the
    /// queued snapshot die with its reader.
    fn prepare(&mut self, activity: WorkerActivity, kind: WorkerStatusKind) -> &mut WorkerStatus {
        if Arc::get_mut(&mut self.status).is_none() {
            self.status = Arc::default();
        }
        let update = Arc::get_mut(&mut self.status)
            .expect("the status allocation is uniquely held after the swap above");
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
        outcome: &mut ExecutionOutcome,
    ) -> Arc<WorkerStatus> {
        let kind = WorkerStatusKind::Completed {
            elapsed_secs: outcome.elapsed_secs,
            executed_node_count: outcome.ran_node_count,
            cancelled: outcome.cancelled,
        };
        let update = self.prepare(activity, kind);
        // The run already reduced itself to one row per node, so publishing is a move:
        // nothing here decides what a node's result was, and nothing downstream has to
        // reconcile a node that arrived twice.
        update.nodes.append(&mut outcome.nodes);
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
            ram: RamUsage::default(),
        });
    }

    pub(crate) fn finish(self) -> Arc<WorkerStatus> {
        debug_assert_eq!(self.status.kind, WorkerStatusKind::Patch);
        debug_assert!(!self.status.nodes.is_empty());
        Arc::clone(self.status)
    }
}
