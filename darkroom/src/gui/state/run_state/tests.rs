use super::*;

use std::collections::BTreeSet;
use std::io;

use scenarium::InvokeError;
use scenarium::LogEntry;
use scenarium::NodeStatus;

use crate::core::wake;
use crate::core::worker_bridge::report_mailbox::ReportMailbox;
use crate::gui::state::preview_store::StoredContent;

fn node_id(n: u128) -> NodeId {
    NodeId::from_u128(n)
}

fn completed_status(executed: &[(NodeId, f64)], errored: &[NodeId]) -> WorkerReport {
    let mut nodes = executed
        .iter()
        .map(|&(node_id, elapsed_secs)| NodeStatus {
            node_id,
            status: Some(NodeExecStatus::Executed { elapsed_secs }),
            ram: RamUsage::default(),
        })
        .collect::<Vec<_>>();
    nodes.extend(errored.iter().map(|&node_id| NodeStatus {
        node_id,
        status: Some(NodeExecStatus::Errored {
            error: RunError::Invoke(Arc::new(InvokeError::external(io::Error::other(
                "test error",
            )))),
        }),
        ram: RamUsage::default(),
    }));
    WorkerReport::Completed(Arc::new(RunSummary {
        executed_node_count: executed.len(),
        nodes,
        ..RunSummary::default()
    }))
}

fn run_state(nodes: impl IntoIterator<Item = NodeId>) -> RunState {
    RunState {
        compiled: Some(Arc::new(CompiledGraph::bare(nodes))),
        ..RunState::new(wake::internals::noop())
    }
}

/// The projection covers exactly the cone the eviction reaches: a node outside
/// it keeps its RAM figure and its preview, because its value is still cached.
#[test]
fn clearing_cache_projections_drops_the_evicted_cone_and_keeps_everything_else() {
    let evicted_node = node_id(1);
    let remaining_node = node_id(2);
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
        .seed(evicted_node, StoredContent::Text("shown".into()));
    state
        .previews
        .seed(remaining_node, StoredContent::Text("kept".into()));

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
        state.previews.get(evicted_node).is_none(),
        "an evicted preview can no longer be re-derived without another run"
    );
    assert!(
        matches!(
            state.previews.get(remaining_node),
            Some(StoredContent::Text(text)) if text == "kept"
        ),
        "a preview outside the cone is still backed by a cached value"
    );
}

/// `clear` drops everything derived from the open document and keeps everything describing the
/// *worker*.
///
/// Node ids are persisted, so keeping the document half would reattach old statuses and previews
/// to a reopened document; dropping the worker half would turn an in-flight run's next report,
/// resolved through `compiled`, into a panic.
#[test]
fn clear_drops_the_document_half_and_keeps_the_worker_half() {
    let node = node_id(1);
    let mut state = run_state([node]);
    let compiled = state.compiled.clone().expect("the fixture installs one");
    state.nodes.entry(node).or_default().status = ExecStatus::Executed(1.5);
    state.nodes.entry(node).or_default().logs.push(NodeLog {
        level: LogLevel::Info,
        message: "from the old document".into(),
    });
    state
        .previews
        .seed(node, StoredContent::Text("stale".into()));
    state.cache_ram = RamUsage { cpu: 24, gpu: 0 };
    state.activity = WorkerActivity::Executing;

    state.clear();

    // The document half: nothing survives to reattach to a reopened file.
    assert_eq!(state.status(node), ExecStatus::None);
    assert!(state.logs(node).is_empty());
    assert!(state.previews.is_empty());

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
    state.apply_report(
        WorkerReport::Progress {
            node_id: node,
            phase: RunPhase::Succeeded { elapsed_secs: 0.5 },
        },
        &mut StatusLog::default(),
    );
    assert_eq!(state.status(node), ExecStatus::Executed(0.5));
}

/// The cache readout follows the worker's own measurements and nothing else: an install carries
/// the resident set left after reconciling, and a clear means the cache is gone, so the status
/// bar does not keep naming a closed document's cache.
#[test]
fn install_and_clear_reports_carry_the_cache_reading() {
    let node = node_id(1);
    let mut state = run_state([node]);
    let mut log = StatusLog::default();
    state.cache_ram = RamUsage { cpu: 24, gpu: 8 };
    state.nodes.entry(node).or_default().status = ExecStatus::Cached;
    state
        .previews
        .seed(node, StoredContent::Text("stale".into()));

    // An install that dropped every cached node reports an empty cache, and
    // the readout follows it down.
    let replacement = Arc::new(CompiledGraph::bare([node_id(2)]));
    state.apply_report(
        WorkerReport::Installed {
            compiled: Arc::clone(&replacement),
            cache_ram: RamUsage::default(),
        },
        &mut log,
    );
    assert_eq!(state.cache_ram, RamUsage::default());
    assert!(
        state
            .compiled
            .as_ref()
            .is_some_and(|c| Arc::ptr_eq(c, &replacement)),
        "the acknowledged program is the one just installed"
    );
    assert_eq!(
        state.status(node),
        ExecStatus::Cached,
        "an install is not a run: it corrects the cache figure and nothing else"
    );

    // A non-empty install is taken just as literally.
    state.apply_report(
        WorkerReport::Installed {
            compiled: replacement,
            cache_ram: RamUsage { cpu: 7, gpu: 3 },
        },
        &mut log,
    );
    assert_eq!(state.cache_ram, RamUsage { cpu: 7, gpu: 3 });

    // A cleared worker holds no program and no cache, and the document half
    // goes with them.
    state.apply_report(WorkerReport::Cleared, &mut log);
    assert_eq!(state.cache_ram, RamUsage::default());
    assert!(state.compiled.is_none());
    assert_eq!(state.status(node), ExecStatus::None);
    assert!(state.previews.is_empty());
}

/// Progress marks a node running, then executed or errored as its lambda finishes, and the
/// worker's activity follows its own reports.
#[test]
fn progress_marks_the_attributed_node_running_then_finished() {
    let (succeeds, fails) = (node_id(1), node_id(2));
    let mut rs = run_state([succeeds, fails]);
    let mut log = StatusLog::default();
    let mut progress = |rs: &mut RunState, node_id, phase| {
        rs.apply_report(WorkerReport::Progress { node_id, phase }, &mut log);
    };

    rs.apply_report(
        WorkerReport::Activity(WorkerActivity::Executing),
        &mut StatusLog::default(),
    );
    assert!(rs.activity.is_executing());
    let at = Instant::now();
    progress(&mut rs, succeeds, RunPhase::Started { at });
    assert_eq!(rs.status(succeeds), ExecStatus::Running(at));
    progress(&mut rs, succeeds, RunPhase::Succeeded { elapsed_secs: 0.5 });
    assert_eq!(rs.status(succeeds), ExecStatus::Executed(0.5));

    progress(&mut rs, fails, RunPhase::Started { at });
    progress(&mut rs, fails, RunPhase::Failed { elapsed_secs: 0.25 });
    assert_eq!(
        rs.status(fails),
        ExecStatus::Errored(None),
        "progress reports the failure before its message"
    );

    // A node no event mentioned stays None.
    assert_eq!(rs.status(node_id(99)), ExecStatus::None);
}

/// The completed snapshot is the authority: it clears the previous run before writing, so a
/// node the new run does not report loses its old status, and one it reports carries its detail
/// inside the status, which live progress replaces together with it.
#[test]
fn completed_snapshot_replaces_the_previous_run() {
    let executed = node_id(1);
    let errored = node_id(2);
    let mut rs = run_state([executed, errored]);

    let mut log = StatusLog::default();
    rs.apply_report(
        completed_status(&[(node_id(1), 1.0), (node_id(2), 0.25)], &[]),
        &mut log,
    );
    assert_eq!(rs.status(executed), ExecStatus::Executed(1.0));
    assert_eq!(rs.status(errored), ExecStatus::Executed(0.25));

    // Second run: only `errored` is reported, and it failed.
    rs.apply_report(completed_status(&[], &[node_id(2)]), &mut log);
    // The failure message rides along with the status — the inspector
    // shows it instead of a bare "errored".
    assert_eq!(
        rs.status(errored),
        ExecStatus::Errored(Some("test error".to_owned()))
    );
    assert_eq!(
        rs.status(executed),
        ExecStatus::None,
        "the previous run's verdict does not survive a snapshot that omits the node"
    );

    // A live re-run succeeds before any snapshot: the message goes with the
    // status it belonged to, rather than lingering under the new one.
    rs.apply_report(
        WorkerReport::Progress {
            node_id: errored,
            phase: RunPhase::Succeeded { elapsed_secs: 0.5 },
        },
        &mut log,
    );
    assert_eq!(rs.status(errored), ExecStatus::Executed(0.5));

    // Third run: `executed` went unfed on its second input.
    rs.apply_report(
        WorkerReport::Completed(Arc::new(RunSummary {
            nodes: vec![NodeStatus {
                node_id: executed,
                status: Some(NodeExecStatus::MissingInputs { ports: vec![1] }),
                ram: RamUsage::default(),
            }],
            ..RunSummary::default()
        })),
        &mut log,
    );
    assert_eq!(rs.status(executed), ExecStatus::MissingInputs(vec![1]));
    assert_eq!(rs.status(errored), ExecStatus::None);

    // Fourth run: a log line goes to the node it names, and a node with no status keeps an entry
    // only for the RAM it holds.
    let holding = node_id(3);
    let idle = node_id(4);
    let mut rs = run_state([executed, errored, holding, idle]);
    rs.apply_report(
        WorkerReport::Completed(Arc::new(RunSummary {
            nodes: [(holding, 7), (idle, 0)]
                .map(|(node_id, cpu)| NodeStatus {
                    node_id,
                    status: None,
                    ram: RamUsage { cpu, gpu: 0 },
                })
                .into(),
            logs: vec![LogEntry {
                node_id: errored,
                level: LogLevel::Warn,
                message: "a warning".into(),
            }],
            ..RunSummary::default()
        })),
        &mut log,
    );
    assert_eq!(
        rs.logs(errored),
        [NodeLog {
            level: LogLevel::Warn,
            message: "a warning".into(),
        }]
    );
    assert!(rs.logs(executed).is_empty(), "a log names one node");
    assert_eq!(rs.ram(holding), RamUsage { cpu: 7, gpu: 0 });
    assert_eq!(
        rs.nodes.keys().copied().collect::<BTreeSet<_>>(),
        BTreeSet::from([errored, holding]),
        "the idle node holds nothing to keep"
    );
}

/// A failed run leaves no results to show, so an execution error clears the
/// last run's statuses. A panicking event lambda stops the loop but fails no
/// run, so the statuses the last completed run left stand. A completed run and a failed one
/// each end a run; the panic ends none.
#[test]
fn only_a_failed_run_clears_the_last_run_statuses() {
    let node = node_id(1);
    let mut rs = run_state([node]);
    let mut log = StatusLog::default();
    rs.apply_report(completed_status(&[(node, 1.0)], &[]), &mut log);
    assert_eq!(rs.runs_ended(), 1);

    rs.apply_report(
        WorkerReport::Error(WorkerError::EventLambdaPanic {
            node_id: node,
            message: "boom".to_owned(),
        }),
        &mut log,
    );
    assert_eq!(rs.status(node), ExecStatus::Executed(1.0));
    assert_eq!(rs.runs_ended(), 1, "the panic ends no run");

    rs.apply_report(
        WorkerReport::Error(WorkerError::Exec {
            error: scenarium::ExecError::NodeSeedNotFound { node_id: node },
        }),
        &mut log,
    );
    assert_eq!(rs.status(node), ExecStatus::None);
    assert_eq!(rs.runs_ended(), 2);
}

/// A node a failure before it stopped reads as skipped, not as failed, so the one node that
/// failed stands out as the cause; the outcome counts each kind and names that node, the first
/// failed one in the run's order, which the rows are in.
#[test]
fn a_skipped_node_is_not_a_failure_and_the_outcome_names_the_cause() {
    let [ran, failed, later_failed, skipped_a, skipped_b] = [1, 2, 3, 4, 5].map(node_id);
    let mut rs = run_state([ran, failed, later_failed, skipped_a, skipped_b]);
    let row = |node_id, status| NodeStatus {
        node_id,
        status: Some(status),
        ram: RamUsage::default(),
    };
    let invoke = || NodeExecStatus::Errored {
        error: RunError::Invoke(Arc::new(InvokeError::external(io::Error::other("boom")))),
    };
    let skipped = || NodeExecStatus::Errored {
        error: RunError::SkippedUpstream,
    };
    let summary = RunSummary {
        nodes: vec![
            row(ran, NodeExecStatus::Executed { elapsed_secs: 0.5 }),
            row(skipped_a, skipped()),
            row(failed, invoke()),
            row(later_failed, invoke()),
            row(skipped_b, skipped()),
        ],
        ..RunSummary::default()
    };
    assert_eq!(rs.outcome(), None, "no run yet");
    rs.apply_report(
        WorkerReport::Completed(Arc::new(summary)),
        &mut StatusLog::default(),
    );
    assert_eq!(
        rs.status(failed),
        ExecStatus::Errored(Some("boom".to_owned()))
    );
    assert_eq!(rs.status(skipped_a), ExecStatus::Skipped);
    assert_eq!(rs.status(skipped_b), ExecStatus::Skipped);
    assert_eq!(
        rs.outcome(),
        Some(RunOutcome {
            ran_count: 1,
            cached_count: 0,
            failed_count: 2,
            skipped_count: 2,
            missing_count: 0,
            cause: Some(failed),
            cancelled: false,
        })
    );
    rs.clear();
    assert_eq!(
        rs.outcome(),
        None,
        "a clear drops the tally with the statuses"
    );
}

/// With no failure, the cause is the first node the run could not feed.
#[test]
fn with_no_failure_the_cause_is_the_first_node_missing_an_input() {
    let [cached, missing] = [1, 2].map(node_id);
    let row = |node_id, status| NodeStatus {
        node_id,
        status: Some(status),
        ram: RamUsage::default(),
    };
    let outcome = RunOutcome::of(&RunSummary {
        nodes: vec![
            row(cached, NodeExecStatus::Cached),
            row(missing, NodeExecStatus::MissingInputs { ports: vec![0] }),
        ],
        cancelled: true,
        ..RunSummary::default()
    });
    assert_eq!(
        outcome,
        RunOutcome {
            ran_count: 0,
            cached_count: 1,
            failed_count: 0,
            skipped_count: 0,
            missing_count: 1,
            cause: Some(missing),
            cancelled: true,
        }
    );
}

/// The run reports of one cycle of a live event loop: it executes, `node` makes progress, and
/// the run ends in `end`, back in the loop.
fn event_run(node: NodeId, end: WorkerReport) -> [WorkerReport; 4] {
    [
        WorkerReport::Activity(WorkerActivity::Executing),
        WorkerReport::Progress {
            node_id: node,
            phase: RunPhase::Succeeded { elapsed_secs: 0.5 },
        },
        end,
        WorkerReport::Activity(WorkerActivity::EventLoop),
    ]
}

fn run_error(node: NodeId) -> WorkerReport {
    WorkerReport::Error(WorkerError::Exec {
        error: scenarium::ExecError::NodeSeedNotFound { node_id: node },
    })
}

fn installed(nodes: [NodeId; 2]) -> WorkerReport {
    WorkerReport::Installed {
        compiled: Arc::new(CompiledGraph::bare(nodes)),
        cache_ram: RamUsage { cpu: 7, gpu: 0 },
    }
}

/// The runs of a live event loop: two that fail and two that end, a structural error, then three
/// more, so each merge rule and the barrier meet the runs on each side of them.
fn event_loop_stream([a, b]: [NodeId; 2]) -> Vec<WorkerReport> {
    let mut stream = vec![installed([a, b])];
    stream.extend(event_run(a, completed_status(&[(a, 1.0)], &[])));
    stream.extend(event_run(b, run_error(b)));
    stream.extend(event_run(a, completed_status(&[(a, 2.0)], &[b])));
    stream.extend(event_run(b, run_error(a)));
    stream.push(WorkerReport::Error(WorkerError::EventLambdaPanic {
        node_id: a,
        message: "boom".to_owned(),
    }));
    stream.extend(event_run(a, run_error(b)));
    stream.extend(event_run(b, completed_status(&[(b, 3.0)], &[])));
    stream.extend(event_run(a, run_error(a)));
    stream
}

/// Each prefix of [`event_loop_stream`] folds through the mailbox to what folding each of its
/// reports leaves: the nodes, the activity, the RAM, the outcome, the status line and the count of
/// runs ended. The whole stream keeps 8 of its 30 reports: on each side of the barrier, the last
/// `Completed`, the error after it and the last `Activity`, with the `Installed` and the barrier:
/// the run 3 `Completed` drops the run 1 one, the run 4 error the run 2 one, and the run 7 error
/// the run 5 one, which leaves 3 superseded runs.
#[test]
fn a_merged_report_stream_folds_as_the_whole_stream() {
    let nodes = [1, 2].map(node_id);
    let whole_len = event_loop_stream(nodes).len();
    assert_eq!(whole_len, 30);
    for prefix_len in 0..=whole_len {
        let mut folded = RunState::new(wake::internals::noop());
        let mut folded_log = StatusLog::default();
        for report in event_loop_stream(nodes).into_iter().take(prefix_len) {
            folded.apply_report(report, &mut folded_log);
        }

        let mailbox = ReportMailbox::default();
        for report in event_loop_stream(nodes).into_iter().take(prefix_len) {
            mailbox.post(report);
        }
        let mut merged = RunState::new(wake::internals::noop());
        let mut merged_log = StatusLog::default();
        let mut kept = VecDeque::new();
        let superseded_runs = mailbox.drain(&mut kept);
        if prefix_len == whole_len {
            assert_eq!(kept.len(), 8);
            assert_eq!(superseded_runs, 3);
        }
        merged.runs_ended += superseded_runs;
        for report in kept {
            merged.apply_report(report, &mut merged_log);
        }

        assert_eq!(
            merged.runs_ended(),
            folded.runs_ended(),
            "prefix {prefix_len}"
        );
        assert_eq!(merged.nodes, folded.nodes, "prefix {prefix_len}");
        assert_eq!(merged.activity, folded.activity, "prefix {prefix_len}");
        assert_eq!(merged.cache_ram, folded.cache_ram, "prefix {prefix_len}");
        assert_eq!(merged.outcome, folded.outcome, "prefix {prefix_len}");
        assert_eq!(
            format!("{:?}", merged_log.cur()),
            format!("{:?}", folded_log.cur()),
            "prefix {prefix_len}"
        );
        if prefix_len == whole_len {
            assert_eq!(folded.runs_ended(), 7);
        }
    }
}

/// A long event loop behind a hidden window keeps the install and the last run alone: 100 runs
/// that each complete leave the `Installed`, the last `Completed` and the last `Activity`, and
/// carry the other 99 runs as a count.
#[test]
fn a_long_event_loop_keeps_its_last_run_alone() {
    let [a, b] = [1, 2].map(node_id);
    let mailbox = ReportMailbox::default();
    mailbox.post(installed([a, b]));
    for _ in 0..100 {
        for report in event_run(a, completed_status(&[(a, 1.0)], &[])) {
            mailbox.post(report);
        }
    }
    let mut kept = VecDeque::new();
    assert_eq!(mailbox.drain(&mut kept), 99);
    assert!(matches!(
        kept.make_contiguous(),
        [
            WorkerReport::Installed { .. },
            WorkerReport::Completed(_),
            WorkerReport::Activity(WorkerActivity::EventLoop),
        ]
    ));
}
