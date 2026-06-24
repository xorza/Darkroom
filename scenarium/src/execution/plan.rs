//! The per-run schedule: the two backward-walk orders plus SoA per-run flag
//! columns. Produced by the [`Planner`](crate::execution::planner::Planner), consumed by
//! the [`Executor`](crate::execution::executor::Executor). Keyed by the same indices as
//! the program's pools. Reused via a buffer on the engine so a repeated run
//! does no scheduling allocation.

/// Per-run scheduling state for one node, indexed by `e_node_idx`.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct NodeFlags {
    pub(crate) wants_execute: bool,
    pub(crate) cached: bool,
    pub(crate) missing_required_inputs: bool,
}

/// Per-run scheduling state for one input, indexed by input-pool index.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct InputFlags {
    pub(crate) missing: bool,
}

#[derive(Debug, Default)]
pub(crate) struct ExecutionPlan {
    /// Post-order DFS over the dependency graph (deps before consumers),
    /// seeded from the terminals. Superset of `execute_order`.
    pub(crate) process_order: Vec<usize>,
    /// Pruned to only nodes whose output is read by an executing consumer.
    pub(crate) execute_order: Vec<usize>,
    /// Per-node flags, indexed by `e_node_idx`.
    pub(crate) node_flags: Vec<NodeFlags>,
    /// Per-input flags, indexed by input-pool index.
    pub(crate) input_flags: Vec<InputFlags>,
    /// Per-output consumer counts, indexed by output-pool index. `> 0` ⇒
    /// the output is Needed this run; `0` ⇒ Skip. A count (not a bool) so
    /// future refcount-based eviction can use the multiplicity.
    pub(crate) output_usage: Vec<u32>,
}

impl ExecutionPlan {
    pub(crate) fn clear(&mut self) {
        self.process_order.clear();
        self.execute_order.clear();
        self.node_flags.clear();
        self.input_flags.clear();
        self.output_usage.clear();
    }

    /// Clear the orders and reset every flag column to default at the given
    /// pool sizes. Called at the start of each planning pass.
    pub(crate) fn reset(&mut self, n_nodes: usize, n_inputs: usize, n_outputs: usize) {
        self.process_order.clear();
        self.execute_order.clear();

        self.node_flags.clear();
        self.node_flags.resize(n_nodes, NodeFlags::default());
        self.input_flags.clear();
        self.input_flags.resize(n_inputs, InputFlags::default());
        self.output_usage.clear();
        self.output_usage.resize(n_outputs, 0);
    }
}
