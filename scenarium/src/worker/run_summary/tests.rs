use super::*;

use crate::execution::error::RunError;
use crate::execution::report::{LogLevel, NodeExecutionStatus};
use crate::graph::identity::{FuncId, NodeId};

/// Publishing a completed run is a move, not a reduction: the rows the run produced
/// reach the host in the order and shape the run gave them, and the only thing the
/// publisher adds is the whole-run header.
#[test]
fn a_summary_publishes_the_runs_rows_verbatim() {
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
        nodes: rows.clone(),
        ran_node_count: 2,
        logs: vec![LogEntry {
            node_id: executed,
            level: LogLevel::Warn,
            message: "warning".into(),
        }],
        cancelled: true,
        cache_ram: RamUsage { cpu: 13, gpu: 17 },
        ..ExecutionOutcome::default()
    };
    let summary = RunSummaryPublisher::default().publish(WorkerActivity::EventLoop, &mut outcome);

    assert_eq!(summary.activity, WorkerActivity::EventLoop);
    assert_eq!((summary.executed_node_count, summary.cancelled), (2, true));
    assert_eq!(summary.cache_ram, RamUsage { cpu: 13, gpu: 17 });
    assert_eq!(summary.logs.len(), 1);
    assert_eq!(summary.logs[0].message, "warning");

    assert_eq!(summary.nodes.len(), rows.len());
    for (published, produced) in summary.nodes.iter().zip(&rows) {
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
                Some(NodeExecutionStatus::Errored { error: a }),
                Some(NodeExecutionStatus::Errored { error: b }),
            ) => assert_eq!(a.to_string(), b.to_string()),
            (None, None) => {}
            (published, produced) => panic!("row changed shape: {produced:?} → {published:?}"),
        }
    }
    assert!(
        outcome.nodes.is_empty() && outcome.logs.is_empty(),
        "the rows moved out of the outcome rather than being copied"
    );
}

/// A summary the host dropped hands its allocation to the next run; one the host still holds is
/// never written into, so the next run publishes into a fresh one and the held one stays as it
/// was.
#[test]
fn a_dropped_summary_is_reused_and_a_held_one_is_left_alone() {
    let row = || NodeStatus {
        node_id: NodeId::unique(),
        status: Some(NodeExecutionStatus::Cached),
        ram: RamUsage::default(),
    };
    let mut publisher = RunSummaryPublisher::default();
    let mut outcome = ExecutionOutcome::default();

    outcome.nodes.push(row());
    let first = publisher.publish(WorkerActivity::Idle, &mut outcome);
    let allocation = Arc::as_ptr(&first);
    drop(first);
    outcome.nodes.push(row());
    let second = publisher.publish(WorkerActivity::Idle, &mut outcome);
    assert_eq!(Arc::as_ptr(&second), allocation);

    outcome.nodes.extend([row(), row()]);
    let third = publisher.publish(WorkerActivity::Idle, &mut outcome);
    assert!(!Arc::ptr_eq(&second, &third));
    assert_eq!((second.nodes.len(), third.nodes.len()), (1, 2));
}
