//! The per-run schedule: the two backward-walk orders plus SoA per-run flag
//! columns. Produced by the [`Planner`](crate::execution::planner::Planner), consumed by
//! the [`Executor`](crate::execution::executor::Executor). Keyed by the same indices as
//! the program's pools. Reused via a buffer on the engine so a repeated run
//! does no scheduling allocation.

use crate::execution::program::{ExecutionBinding, ExecutionInput};

/// The planner's verdict for one node this run, indexed by `e_node_idx`. The three
/// states are mutually exclusive *by construction* — unlike the prior three-bool
/// struct, which could represent contradictions (`cached && wants_execute`) that
/// `validate` had to assert away. The default (`MissingInputs`) is the conservative
/// "not yet established as runnable" value for nodes outside `process_order`, whose
/// verdict is never read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum NodeVerdict {
    /// Served from cache (resident or disk-available) — not executed.
    Cached,
    /// Runnable this round: scheduled into `execute_order`.
    Execute,
    /// A required input is unsatisfied (unbound, or fed by a non-runnable producer);
    /// can't run, and the "missing" verdict propagates to its consumers.
    #[default]
    MissingInputs,
}

impl NodeVerdict {
    pub(crate) fn is_cached(self) -> bool {
        self == NodeVerdict::Cached
    }
    pub(crate) fn wants_execute(self) -> bool {
        self == NodeVerdict::Execute
    }
    pub(crate) fn missing_required_inputs(self) -> bool {
        self == NodeVerdict::MissingInputs
    }
}

/// Whether one input is unsatisfied: an unbound *required* port, or a bind to a
/// producer that itself can't run (missing propagates only through non-runnable
/// producers — a cached or executing one delivers a value, optional or not).
/// `verdicts` must already hold the producer's verdict, which the planner's
/// post-order forward pass guarantees. Shared by that pass and the executor's
/// stats so the two can't drift.
pub(crate) fn input_missing(input: &ExecutionInput, verdicts: &[NodeVerdict]) -> bool {
    match &input.binding {
        ExecutionBinding::None => input.required,
        ExecutionBinding::Const(_) => false,
        ExecutionBinding::Bind(addr) => verdicts[addr.target_idx].missing_required_inputs(),
    }
}

#[derive(Debug, Default)]
pub(crate) struct ExecutionPlan {
    /// Post-order DFS over the dependency graph (deps before consumers),
    /// seeded from the terminals. Superset of `execute_order`.
    pub(crate) process_order: Vec<usize>,
    /// Pruned to only nodes whose output is read by an executing consumer.
    pub(crate) execute_order: Vec<usize>,
    /// Per-node verdict (cached / execute / missing-inputs), indexed by `e_node_idx`.
    pub(crate) verdicts: Vec<NodeVerdict>,
    /// Per-output consumer counts, indexed by output-pool index. `> 0` ⇒ the output
    /// is `Needed` this run; `0` ⇒ `Skip`. The executor passes the count through to
    /// each lambda as [`OutputUsage`](crate::func_lambda::OutputUsage) so a node can
    /// skip computing outputs nobody reads.
    pub(crate) output_usage: Vec<u32>,
}

impl ExecutionPlan {
    pub(crate) fn clear(&mut self) {
        self.process_order.clear();
        self.execute_order.clear();
        self.verdicts.clear();
        self.output_usage.clear();
    }

    /// Clear the orders and reset every per-node verdict to default at the given
    /// pool sizes. Called at the start of each planning pass.
    pub(crate) fn reset(&mut self, n_nodes: usize, n_outputs: usize) {
        self.process_order.clear();
        self.execute_order.clear();

        self.verdicts.clear();
        self.verdicts.resize(n_nodes, NodeVerdict::default());
        self.output_usage.clear();
        self.output_usage.resize(n_outputs, 0);
    }
}
