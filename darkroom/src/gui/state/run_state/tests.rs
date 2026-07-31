use super::*;

use scenarium::CompiledGraphBuilder;
use scenarium::FuncId;
use scenarium::{LogLevel, NodeStatus};

use crate::gui::state::preview_store::StoredContent;

fn nid(n: u128) -> NodeId {
    NodeId::from_u128(n)
}

fn completed_status(executed: &[(NodeId, f64)], errored: &[NodeId]) -> WorkerStatus {
    let mut nodes = executed
        .iter()
        .map(|&(node_id, elapsed_secs)| NodeStatus {
            node_id,
            status: Some(NodeExecutionStatus::Executed { elapsed_secs }),
            ram: RamUsage::default(),
        })
        .collect::<Vec<_>>();
    nodes.extend(errored.iter().map(|&node_id| NodeStatus {
        node_id,
        status: Some(NodeExecutionStatus::Errored {
            elapsed_secs: None,
            error: RunError::Invoke {
                func_id: FuncId::from_u128(0),
                message: "test error".into(),
            },
        }),
        ram: RamUsage::default(),
    }));
    WorkerStatus {
        kind: WorkerStatusKind::Completed {
            elapsed_secs: 0.0,
            executed_node_count: executed.len(),
            cancelled: false,
        },
        nodes,
        ..WorkerStatus::default()
    }
}

fn node_patch(
    activity: WorkerActivity,
    node_id: NodeId,
    status: NodeExecutionStatus,
) -> WorkerStatus {
    WorkerStatus {
        activity,
        kind: WorkerStatusKind::Patch,
        nodes: vec![NodeStatus {
            node_id,
            status: Some(status),
            ram: RamUsage::default(),
        }],
        ..WorkerStatus::default()
    }
}

fn run_state(nodes: impl IntoIterator<Item = NodeId>) -> RunState {
    let mut builder = CompiledGraphBuilder::new();
    for node_id in nodes {
        builder.insert_node(node_id);
    }
    RunState {
        compiled: Some(builder.build()),
        ..RunState::default()
    }
}

#[test]
fn clearing_cache_projections_drops_ram_and_pins_but_keeps_run_results() {
    let evicted_node = nid(1);
    let remaining_node = nid(2);
    let mut state = run_state([evicted_node, remaining_node]);
    state.nodes.entry(evicted_node).or_default().status = ExecStatus::Cached;
    state.nodes.entry(evicted_node).or_default().ram = RamUsage { cpu: 11, gpu: 0 };
    state
        .nodes
        .entry(remaining_node)
        .or_default()
        .logs
        .push(NodeLog {
            level: LogLevel::Info,
            message: "kept".into(),
        });
    state.nodes.entry(remaining_node).or_default().ram = RamUsage { cpu: 13, gpu: 0 };
    state.cache_ram = RamUsage { cpu: 24, gpu: 0 };
    state
        .previews
        .entries
        .insert(evicted_node, StoredContent::Text("shown".into()));
    state
        .previews
        .entries
        .insert(remaining_node, StoredContent::Text("kept".into()));

    state.clear_cache_projections();

    assert_eq!(state.cache_ram, RamUsage::default());
    assert_eq!(state.ram(evicted_node), RamUsage::default());
    assert_eq!(state.ram(remaining_node), RamUsage::default());
    assert_eq!(state.status(evicted_node), ExecStatus::Cached);
    assert_eq!(state.nodes[&remaining_node].logs[0].message, "kept");
    assert!(
        state.previews.entries.is_empty(),
        "a preview's value is a run result too, and goes with the rest"
    );
}

#[test]
fn node_patch_marks_the_attributed_node_running_then_executed() {
    let node = nid(1);
    let mut rs = run_state([node]);

    rs.apply_worker_status(&node_patch(
        WorkerActivity::Executing,
        node,
        NodeExecutionStatus::Running { at: Instant::now() },
    ));
    assert!(matches!(rs.status(node), ExecStatus::Running(_)));

    rs.apply_worker_status(&node_patch(
        WorkerActivity::Executing,
        node,
        NodeExecutionStatus::Executed { elapsed_secs: 0.5 },
    ));
    assert_eq!(rs.status(node), ExecStatus::Executed(0.5));

    // A node no event mentioned stays None.
    assert_eq!(rs.status(nid(99)), ExecStatus::None);
}

/// The completed snapshot is the authority: it clears the previous run
/// before writing, so a node the new run says nothing about loses the
/// status the old one gave it, and one it does report carries the failure
/// message alongside the status.
#[test]
fn completed_snapshot_replaces_the_previous_run() {
    let executed = nid(1);
    let errored = nid(2);
    let mut rs = run_state([executed, errored]);

    rs.apply_worker_status(&completed_status(&[(nid(1), 1.0), (nid(2), 0.25)], &[]));
    assert_eq!(rs.status(executed), ExecStatus::Executed(1.0));
    assert_eq!(rs.status(errored), ExecStatus::Executed(0.25));
    assert_eq!(rs.error(errored), None);

    // Second run: only `errored` is reported, and it failed.
    rs.apply_worker_status(&completed_status(&[], &[nid(2)]));
    assert_eq!(rs.status(errored), ExecStatus::Errored);
    // The failure message rides along with the status — the inspector
    // shows it instead of a bare "errored".
    assert_eq!(rs.error(errored), Some("test error"));
    assert_eq!(
        rs.status(executed),
        ExecStatus::None,
        "the previous run's verdict does not survive a snapshot that omits the node"
    );
}
