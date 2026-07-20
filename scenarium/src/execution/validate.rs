//! Debug-only self-consistency checks for the compiled program, the runtime
//! cache, and the per-run plan. Every entry point is an `is_debug()`-gated no-op
//! in release; in debug it asserts invariants the pipeline is supposed to
//! maintain, so a logic bug trips here at its source rather than surfacing as
//! corrupted output downstream.

use common::is_debug;
use hashbrown::HashSet;

use crate::execution::cache::RuntimeCache;
use crate::execution::compile::CompiledGraph;
use crate::execution::plan::ExecutionPlan;
use crate::execution::program::{ExecutionBinding, ExecutionProgram};
use crate::library::Library;

impl CompiledGraph {
    /// Self-consistency of the freshly compiled artifact against the `Library`
    /// it was compiled from. The source graph is gone after flattening, so this
    /// validates each `e_node` against its func and checks binding integrity.
    /// Runs at compile (where the library is in hand); the library-free
    /// install-side checks live in [`Self::validate_installed`].
    pub(crate) fn validate(&self, library: &Library) {
        if !is_debug() {
            return;
        }

        let program = &self.program;
        for e_node in program.e_nodes.values() {
            // A special node's interface is its hardcoded spec, not a library func.
            let func = match e_node.special {
                Some(s) => s.func(),
                None => library.by_id(&e_node.func_id).unwrap(),
            };
            assert_eq!(e_node.inputs.len as usize, func.inputs.len());
            assert_eq!(e_node.outputs.len as usize, func.outputs.len());
            assert_eq!(e_node.events.len as usize, func.events.len());

            for e_input in program.node_inputs(e_node) {
                if let ExecutionBinding::Bind(e_addr) = &e_input.binding {
                    let target = &program.e_nodes[&e_addr.target];
                    assert!(e_addr.port_idx < target.outputs.len as usize);
                }
            }
        }
    }

    /// The engine's runtime `cache` has exactly this artifact's node ids after
    /// `reconcile` — the install-side half of the checks;
    /// artifact-vs-library consistency runs at compile ([`Self::validate`]).
    pub(crate) fn validate_installed(&self, cache: &RuntimeCache) {
        if !is_debug() {
            return;
        }

        assert_eq!(cache.slots.len(), self.program.e_nodes.len());

        for node_id in self.program.e_nodes.keys() {
            let e_node = &self.program.e_nodes[node_id];
            let slot = &cache.slots[node_id];
            if let Some(output_values) = slot.output_values() {
                assert_eq!(output_values.len(), e_node.outputs.len as usize);
            }
        }
    }
}

/// A planned schedule is well-formed: `process_order` is a post-order DFS (unique, every
/// Bind dep before its consumer) and all binding addresses are in range.
pub(crate) fn schedule(program: &ExecutionProgram, plan: &ExecutionPlan) {
    if !is_debug() {
        return;
    }

    assert!(plan.process_order.len() <= program.e_nodes.len());

    // `process_order` is a post-order DFS: unique, and every Bind dep appears
    // before its consumer.
    let mut seen_in_order = HashSet::with_capacity(program.e_nodes.len());
    for &node_id in &plan.process_order {
        assert!(program.e_nodes.contains_key(&node_id));
        for input in program.node_inputs(&program.e_nodes[&node_id]) {
            // Every Bind dep must be earlier in the order (bounds are re-checked, with
            // the port index, in the all-nodes loop below).
            if let ExecutionBinding::Bind(addr) = &input.binding {
                assert!(seen_in_order.contains(&addr.target));
            }
        }
        assert!(seen_in_order.insert(node_id));
    }

    // Every pinned preview node is also a walk root.
    for idx in &plan.pinned {
        assert!(plan.roots.contains(idx));
    }

    // Per-node verdict consistency is unrepresentable by construction (`NodeVerdict` is a
    // plain enum of mutually exclusive states), so only binding bounds remain to check.
    for e_node in program.e_nodes.values() {
        for e_input in program.node_inputs(e_node) {
            if let ExecutionBinding::Bind(addr) = &e_input.binding {
                assert!(addr.port_idx < program.e_nodes[&addr.target].outputs.len as usize);
            }
        }
    }
}
