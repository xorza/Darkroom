//! Debug-only self-consistency checks for the compiled program, the runtime
//! cache, and the per-run plan. Every entry point is an `is_debug()`-gated no-op
//! in release; in debug it asserts invariants the pipeline is supposed to
//! maintain, so a logic bug trips here at its source rather than surfacing as
//! corrupted output downstream.

use common::is_debug;
use hashbrown::HashSet;

use crate::execution::cache::RuntimeCache;
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

    // Runtime slots stay index-aligned to nodes after `reconcile`.
    assert_eq!(cache.slots.len(), program.e_nodes.len());

    let mut seen_node_ids: HashSet<NodeId> = HashSet::with_capacity(program.e_nodes.len());
    for (idx, e_node) in program.e_nodes.iter().enumerate() {
        assert!(seen_node_ids.insert(e_node.id));

        let slot = &cache.slots[idx];
        assert_eq!(slot.id, e_node.id);
        if let Some(output_values) = slot.output_values() {
            assert_eq!(output_values.len(), e_node.outputs.len as usize);
        }

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
                assert!(e_addr.target_idx.idx() < program.e_nodes.len());
                let target = &program.e_nodes[e_addr.target_idx];
                assert!(e_addr.port_idx < target.outputs.len as usize);
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
