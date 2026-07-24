use std::time::Instant;

use crate::execution::cache::runtime::RuntimeCache;
use crate::execution::error::RunError;
use crate::execution::identity::{ExecutionInputPort, ExecutionNodeId};
use crate::execution::outcome::{ExecutedNodeOutcome, ExecutionOutcome, NodeError};
use crate::execution::plan::{ExecutionPlan, input_missing};
use crate::execution::program::index::NodeMap;
use crate::execution::program::{ExecutionBinding, ExecutionProgram};

/// What became of a node this run — the single per-node result map, so the run-time
/// facts can't contradict (a node can't be `Reused` yet carry a run time, or `Ran` yet
/// also flagged errored). Carries its own `RunError`/elapsed, so nothing lives in a side
/// map.
#[derive(Debug, Clone, Default)]
pub(crate) enum NodeOutcome {
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
    /// Never ran — an upstream dependency errored or its func has no implementation attached.
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
/// [`RunError::SkippedUpstream`] for an errored dependency or
/// [`RunError::MissingLambda`] for a func with no implementation.
pub(crate) fn mark_skipped(
    cache: &mut RuntimeCache,
    outcomes: &mut NodeMap<NodeOutcome>,
    e_node_id: ExecutionNodeId,
    error: RunError,
) {
    cache.slots.get_mut(&e_node_id).unwrap().clear_output();
    *outcomes.get_mut(&e_node_id).unwrap() = NodeOutcome::Skipped { error };
}

pub(crate) fn has_errored_dependency(
    program: &ExecutionProgram,
    outcomes: &NodeMap<NodeOutcome>,
    e_node_id: ExecutionNodeId,
) -> bool {
    program
        .node_inputs(&program.e_nodes[&e_node_id])
        .iter()
        .any(|input| {
            matches!(&input.binding, ExecutionBinding::Bind(addr) if outcomes[&addr.e_node_id].error().is_some())
        })
}

pub(crate) fn collect_execution_outcome(
    program: &ExecutionProgram,
    plan: &ExecutionPlan,
    outcomes: &NodeMap<NodeOutcome>,
    start: Instant,
    outcome: &mut ExecutionOutcome,
) {
    // The schedule (and its per-node outcomes) is `process_order`. Each node's outcome is
    // the sole source of truth; a node the run never reached (a cancelled run's tail, or
    // skipped for missing inputs) is `Pending` and contributes to no list here.
    for &e_node_id in &plan.process_order {
        let e = &program.e_nodes[&e_node_id];
        match &outcomes[&e_node_id] {
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
        if plan.verdicts[&e_node_id].missing_required_inputs() {
            // Recompute which ports are unsatisfied (shares `input_missing` with the
            // planner) — only for the rare missing node, so it isn't worth a stored column.
            for (i, input) in program.node_inputs(e).iter().enumerate() {
                if input_missing(input, &plan.verdicts) {
                    outcome.missing_inputs.push(ExecutionInputPort {
                        e_node_id,
                        port_idx: i,
                    });
                }
            }
        }
        if let Some(err) = outcomes[&e_node_id].error() {
            outcome.node_errors.push(NodeError {
                e_node_id,
                error: err.clone(),
            });
        }
    }
    outcome.elapsed_secs = start.elapsed().as_secs_f64();
}
