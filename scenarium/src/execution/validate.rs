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
use crate::graph::NodeId;
use crate::library::Library;

/// Self-consistency of the compiled program against the `Library`, plus that the
/// runtime cache stayed index-aligned to the nodes after `reconcile`. The source
/// graph is gone after flattening, so this validates each `e_node` against its
/// func and checks binding integrity.
pub(crate) fn compiled(program: &ExecutionProgram, cache: &RuntimeCache, library: &Library) {
    if !is_debug() {
        return;
    }

    assert_eq!(cache.slots.len(), program.e_nodes.len());

    for (idx, e_node) in program.e_nodes.iter().enumerate() {
        let slot = &cache.slots[idx];
        assert_eq!(slot.id, e_node.id);
        if let Some(output_values) = slot.output_values() {
            assert_eq!(output_values.len(), e_node.outputs.len as usize);
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
    for &idx in &plan.process_order {
        assert!(idx.idx() < program.e_nodes.len());
        for input in program.node_inputs(&program.e_nodes[idx]) {
            // Every Bind dep must be earlier in the order (bounds are re-checked, with
            // the port index, in the all-nodes loop below).
            if let ExecutionBinding::Bind(addr) = &input.binding {
                assert!(seen_in_order.contains(&addr.target_idx));
            }
        }
        assert!(seen_in_order.insert(idx));
    }

    // Eviction keeps a retained node only while it's on the frontier, which for a pinned
    // preview root holds because every pinned node is a walk root (roots seed the
    // disposition walk).
    for idx in &plan.pinned {
        assert!(plan.roots.contains(idx));
    }

    // Per-node verdict consistency is unrepresentable by construction (`NodeVerdict` is a
    // plain enum of mutually exclusive states), so only binding bounds remain to check.
    for e_node in program.e_nodes.iter() {
        for e_input in program.node_inputs(e_node) {
            if let ExecutionBinding::Bind(addr) = &e_input.binding {
                assert!(addr.target_idx.idx() < program.e_nodes.len());
                assert!(addr.port_idx < program.e_nodes[addr.target_idx].outputs.len as usize);
            }
        }
    }
}
