use std::sync::Arc;

use crate::RamUsage;
use crate::execution::report::LogEntry;
use crate::execution::report::{ExecutionOutcome, NodeExecutionStatus, NodeStatus};
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
            node_id: progress.node_id,
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

#[cfg(test)]
mod tests {
    use super::*;

    use crate::execution::error::RunError;
    use crate::execution::report::{LogEntry, LogLevel, NodeExecutionStatus, NodeStatus};
    use crate::graph::identity::{FuncId, NodeId};

    /// Publishing a completed run is a move, not a reduction: the rows the run produced
    /// reach the GUI in the order and shape the run gave them, and the only thing the
    /// publisher adds is the whole-run header.
    #[test]
    fn completed_status_publishes_the_runs_rows_verbatim() {
        let executed = NodeId::unique();
        let missing = NodeId::unique();
        let failed = NodeId::unique();
        let resident = NodeId::unique();
        let rows = vec![
            NodeStatus {
                node_id: executed,
                status: Some(NodeExecutionStatus::Executed { elapsed_secs: 0.5 }),
                ram: RamUsage { cpu: 3, gpu: 0 },
            },
            NodeStatus {
                node_id: missing,
                status: Some(NodeExecutionStatus::MissingInputs { ports: vec![1, 3] }),
                ram: RamUsage::default(),
            },
            NodeStatus {
                node_id: failed,
                status: Some(NodeExecutionStatus::Errored {
                    elapsed_secs: Some(0.25),
                    error: RunError::Invoke {
                        func_id: FuncId::unique(),
                        message: "failed".into(),
                    },
                }),
                ram: RamUsage::default(),
            },
            NodeStatus {
                node_id: resident,
                status: None,
                ram: RamUsage { cpu: 5, gpu: 7 },
            },
        ];
        let mut outcome = ExecutionOutcome {
            elapsed_secs: 1.25,
            nodes: rows.clone(),
            ran_node_count: 2,
            triggered_events: Vec::new(),
            event_triggers: Vec::new(),
            logs: vec![LogEntry {
                node_id: executed,
                level: LogLevel::Warn,
                message: "warning".into(),
            }],
            cancelled: true,
            cache_ram: RamUsage { cpu: 13, gpu: 17 },
        };
        let mut publisher = WorkerStatusPublisher::default();
        let status = publisher.completed(WorkerActivity::EventLoop, &mut outcome);

        assert_eq!(status.activity, WorkerActivity::EventLoop);
        assert_eq!(
            status.kind,
            WorkerStatusKind::Completed {
                elapsed_secs: 1.25,
                executed_node_count: 2,
                cancelled: true,
            }
        );
        assert_eq!(status.cache_ram, RamUsage { cpu: 13, gpu: 17 });
        assert_eq!(status.logs.len(), 1);
        assert_eq!(status.logs[0].message, "warning");

        assert_eq!(status.nodes.len(), rows.len());
        for (published, produced) in status.nodes.iter().zip(&rows) {
            assert_eq!(published.node_id, produced.node_id);
            assert_eq!(published.ram, produced.ram);
            match (&published.status, &produced.status) {
                (
                    Some(NodeExecutionStatus::Executed { elapsed_secs: a }),
                    Some(NodeExecutionStatus::Executed { elapsed_secs: b }),
                ) => assert_eq!(a, b),
                (
                    Some(NodeExecutionStatus::MissingInputs { ports: a }),
                    Some(NodeExecutionStatus::MissingInputs { ports: b }),
                ) => assert_eq!(a, b, "the exact unfed ports survive publication"),
                (
                    Some(NodeExecutionStatus::Errored {
                        elapsed_secs: a,
                        error: ea,
                    }),
                    Some(NodeExecutionStatus::Errored {
                        elapsed_secs: b,
                        error: eb,
                    }),
                ) => {
                    assert_eq!(a, b, "a failure keeps the time its attempt cost");
                    assert_eq!(ea.to_string(), eb.to_string());
                }
                (None, None) => {}
                (published, produced) => panic!("row changed shape: {produced:?} → {published:?}"),
            }
        }
        assert!(
            outcome.nodes.is_empty(),
            "the rows moved out of the outcome rather than being copied"
        );
    }
}
