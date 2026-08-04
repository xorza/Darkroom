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

/// The projection covers exactly the cone the eviction reaches: a node outside
/// it keeps its RAM figure and its preview, because its value is still cached.
#[test]
fn clearing_cache_projections_drops_the_evicted_cone_and_keeps_everything_else() {
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

    state.clear_cache_projections(&[evicted_node]);

    // The global total deduplicates shared values, so it cannot be projected
    // from the per-node figures and reads as unknown until the next run.
    assert_eq!(state.cache_ram, RamUsage::default());
    assert_eq!(state.ram(evicted_node), RamUsage::default());
    assert_eq!(
        state.ram(remaining_node),
        RamUsage { cpu: 13, gpu: 0 },
        "a node outside the cone still holds its value"
    );
    assert_eq!(state.status(evicted_node), ExecStatus::Cached);
    assert_eq!(state.nodes[&remaining_node].logs[0].message, "kept");
    assert!(
        !state.previews.entries.contains_key(&evicted_node),
        "an evicted preview can no longer be re-derived without another run"
    );
    assert!(
        matches!(
            state.previews.entries.get(&remaining_node),
            Some(StoredContent::Text(text)) if text == "kept"
        ),
        "a preview outside the cone is still backed by a cached value"
    );
}

/// `clear` splits the two halves exactly where their owners do: everything
/// derived from the open document goes, everything describing the *worker*
/// stays.
///
/// Both sides are load-bearing for `App::adopt_document`. Node ids are
/// persisted, so leaving the document half would reattach the previous
/// session's statuses and preview images to a reopened document that has not
/// run. Dropping the worker half would be worse: an in-flight run's next patch
/// resolves through `compiled`, and clearing it turns that report into a panic.
#[test]
fn clear_drops_the_document_half_and_keeps_the_worker_half() {
    let node = nid(1);
    let mut state = run_state([node]);
    let compiled = state.compiled.clone().expect("the fixture installs one");
    state.nodes.entry(node).or_default().status = ExecStatus::Executed(1.5);
    state.nodes.entry(node).or_default().logs.push(NodeLog {
        level: LogLevel::Info,
        message: "from the old document".into(),
    });
    state
        .previews
        .entries
        .insert(node, StoredContent::Text("stale".into()));
    state.cache_ram = RamUsage { cpu: 24, gpu: 0 };
    state.activity = WorkerActivity::Executing;

    state.clear();

    // The document half: nothing survives to reattach to a reopened file.
    assert_eq!(state.status(node), ExecStatus::None);
    assert!(state.logs(node).is_empty());
    assert!(state.previews.entries.is_empty());

    // The worker half: the acknowledged program still resolves later reports,
    // the run is still reported as in flight, and the cache still holds what
    // it holds.
    assert!(
        state
            .compiled
            .as_ref()
            .is_some_and(|c| Arc::ptr_eq(c, &compiled)),
        "the acknowledged program is untouched"
    );
    assert!(state.activity.is_executing(), "the run is still in flight");
    assert_eq!(state.cache_ram, RamUsage { cpu: 24, gpu: 0 });

    // And that program still attributes a report, rather than panicking.
    state.apply_worker_status(&node_patch(
        WorkerActivity::Executing,
        node,
        NodeExecutionStatus::Cached,
    ));
    assert_eq!(state.status(node), ExecStatus::Cached);
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
