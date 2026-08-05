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
