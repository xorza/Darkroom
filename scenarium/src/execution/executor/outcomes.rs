use std::time::Instant;

use crate::execution::cache::runtime::RuntimeCache;
use crate::execution::error::RunError;
use crate::execution::identity::ExecutionInputPort;
use crate::execution::outcome::{ExecutedNodeOutcome, ExecutionOutcome, NodeError};
use crate::execution::plan::{RunSchedule, input_missing};
use crate::execution::program::index::{NodeColumn, NodeIdx};
use crate::execution::program::{ExecutionBinding, Program};

/// What became of a node this run — the single per-node result map, so the run-time
/// facts can't contradict (a node can't be `Reused` yet carry a run time, or `Ran` yet
/// also flagged errored). Carries its own `RunError`/elapsed, so nothing lives in a side
/// map.
#[derive(Debug, Clone, Default)]
pub(super) enum NodeOutcome {
    /// Not reached this run: skipped for missing inputs, below a cancel, or unscheduled.
    #[default]
    Pending,
    /// Served from a RAM/disk cache under an unchanged digest — counted as cached.
    Reused,
    /// Pruned by the pre-run cut: every consumer that would read this node reused a cache,
    /// so its output is never read and its lambda is skipped. `cached` is whether its output
    /// remains resident; unprobed disk blobs are not runtime cache state.
    Cut { cached: bool },
    /// Its lambda ran and succeeded, taking `secs`.
    Ran { secs: f64 },
    /// Its lambda ran but errored — an invoke failure, or a cancel mid-invoke.
    Failed { secs: f64, error: RunError },
    /// Never ran — an upstream dependency errored, its func has no implementation attached,
    /// or the cached output it was resolved to reuse failed to load.
    Skipped { error: RunError },
}

impl NodeOutcome {
    /// The run error the node carries — a failed run, or a node skipped for an error.
    fn error(&self) -> Option<&RunError> {
        match self {
            NodeOutcome::Failed { error, .. } | NodeOutcome::Skipped { error } => Some(error),
            _ => None,
        }
    }
}

/// Drop `e_node_id` from this run: clear any stale cached output so it isn't served as
/// this run's result, and record the outcome under the caller's reason —
/// [`RunError::SkippedUpstream`] for an errored dependency,
/// [`RunError::MissingLambda`] for a func with no implementation, or
/// [`RunError::CacheLoadFailed`] for a probed blob that no longer loads.
pub(super) fn mark_skipped(
    cache: &mut RuntimeCache,
    outcomes: &mut NodeColumn<NodeOutcome>,
    node_idx: NodeIdx,
    error: RunError,
) {
    cache[node_idx].clear_output();
    outcomes[node_idx] = NodeOutcome::Skipped { error };
}

pub(super) fn has_errored_dependency(
    program: &Program,
    outcomes: &NodeColumn<NodeOutcome>,
    node_idx: NodeIdx,
) -> bool {
    program.inputs[program[node_idx].inputs]
        .iter()
        .any(|input| {
            matches!(&input.binding, ExecutionBinding::Bind(addr) if outcomes[addr.node_idx].error().is_some())
        })
}

pub(super) fn collect_execution_outcome(
    program: &Program,
    schedule: &RunSchedule,
    outcomes: &NodeColumn<NodeOutcome>,
    start: Instant,
    outcome: &mut ExecutionOutcome,
) {
    // The schedule (and its per-node outcomes) is `process_order`. Each node's outcome is
    // the sole source of truth; a node the run never reached (a cancelled run's tail, or
    // skipped for missing inputs) is `Pending` and contributes to no list here.
    for &node_idx in &schedule.process_order {
        let e_node = &program[node_idx];
        let e_node_id = program.e_node_ids[node_idx];
        match &outcomes[node_idx] {
            // A reuse hit, or a node the cut pruned that still holds a resident value, are
            // both "available, not recomputed" — reported cached. A pruned node
            // (`Cut { cached: false }`) has no value this run and falls through, uncounted.
            NodeOutcome::Reused | NodeOutcome::Cut { cached: true } => {
                outcome.cached_nodes.push(e_node_id);
            }
            NodeOutcome::Ran { secs } => outcome.executed_nodes.push(ExecutedNodeOutcome {
                e_node_id,
                elapsed_secs: *secs,
            }),
            // A cancelled invoke didn't complete — omit it from the executed set so the
            // consumer doesn't paint it as executed (its error still lands below). A
            // genuine failure did run; it appears in both lists.
            NodeOutcome::Failed { secs, error } if !matches!(error, RunError::Cancelled { .. }) => {
                outcome.executed_nodes.push(ExecutedNodeOutcome {
                    e_node_id,
                    elapsed_secs: *secs,
                });
            }
            _ => {}
        }
        if schedule.states[node_idx].missing_required_inputs() {
            // Recompute which ports are unsatisfied (shares `input_missing` with the
            // planner) — only for the rare missing node, so it isn't worth a stored column.
            for (i, input) in program.inputs[e_node.inputs].iter().enumerate() {
                if input_missing(input, &schedule.states) {
                    outcome.missing_inputs.push(ExecutionInputPort {
                        e_node_id,
                        port_idx: i,
                    });
                }
            }
        }
        if let Some(err) = outcomes[node_idx].error() {
            outcome.node_errors.push(NodeError {
                e_node_id,
                error: err.clone(),
            });
        }
    }
    outcome.elapsed_secs = start.elapsed().as_secs_f64();
}
