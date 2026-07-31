//! Compile-owned validation of the finished artifact.
//!
//! Linking constructs the artifact, then this stage checks it while the
//! authoring [`Library`] is still in hand. Keeping that dependency here means
//! the installed [`CompiledGraph`] knows neither the compiler nor the library;
//! it owns runtime data and queries only.

use ::common::is_debug;

use crate::common::column::Idx;
use crate::execution::compile::error::CompiledGraphValidationError;
use crate::execution::compiled::CompiledGraph;
use crate::execution::compiled::ExecutionBinding;
use crate::library::Library;

/// Self-consistency of a freshly linked artifact against the library it was
/// compiled from. The source graph is gone after lowering, so this validates
/// each execution node against its function and checks the linked ranges and
/// addresses.
pub(super) fn validate(
    compiled: &CompiledGraph,
    library: &Library,
) -> Result<(), CompiledGraphValidationError> {
    let program = &compiled;
    for (node_id, e_node) in program.node_ids.iter().zip(program.e_nodes.iter()) {
        if e_node.func_id.is_nil() {
            return Err(CompiledGraphValidationError::NilFuncId { node_id: *node_id });
        }
        // A special node's interface is its hardcoded spec, not a library func.
        let func =
            match e_node.special {
                Some(special) => special.func(),
                None => library.by_id(e_node.func_id).ok_or(
                    CompiledGraphValidationError::MissingFunc {
                        node_id: *node_id,
                        func_id: e_node.func_id,
                    },
                )?,
            };
        if e_node.inputs.len as usize != func.inputs.len() {
            return Err(CompiledGraphValidationError::InputArity { node_id: *node_id });
        }
        if e_node.outputs.len as usize != func.outputs.len() {
            return Err(CompiledGraphValidationError::OutputArity { node_id: *node_id });
        }
        if e_node.events.len as usize != func.events.len() {
            return Err(CompiledGraphValidationError::EventArity { node_id: *node_id });
        }
        let inputs = program
            .inputs
            .get_span(e_node.inputs)
            .ok_or(CompiledGraphValidationError::InputRange { node_id: *node_id })?;
        if program.outputs.get_span(e_node.outputs).is_none() {
            return Err(CompiledGraphValidationError::OutputRange { node_id: *node_id });
        }
        let events = program
            .events
            .get_span(e_node.events)
            .ok_or(CompiledGraphValidationError::EventRange { node_id: *node_id })?;

        // Unreachable while `wire_subscriptions` mints every subscriber from
        // a successful id lookup — kept as the backstop if that stops holding.
        for e_event in events {
            if let Some(&subscriber) = e_event
                .subscribers
                .iter()
                .find(|s| s.idx() >= program.e_nodes.len())
            {
                return Err(CompiledGraphValidationError::MissingEventSubscriber {
                    node_id: *node_id,
                    subscriber,
                });
            }
        }

        for e_input in inputs {
            if let ExecutionBinding::Bind(e_addr) = &e_input.binding {
                // Unreachable while `intern_bindings` mints every address from
                // a successful id lookup — kept as the backstop if that stops
                // holding.
                let target_e_node = program.e_nodes.get(e_addr.node_idx).ok_or(
                    CompiledGraphValidationError::MissingBindingTarget {
                        node_id: *node_id,
                        target: *e_addr,
                    },
                )?;
                if e_addr.port_idx >= target_e_node.outputs.len {
                    return Err(CompiledGraphValidationError::BindingOutputOutOfRange {
                        node_id: *node_id,
                        target: *e_addr,
                    });
                }
            }
        }
    }
    Ok(())
}

/// Debug-only assert form of [`validate`].
pub(super) fn validate_debug(compiled: &CompiledGraph, library: &Library) {
    if !is_debug() {
        return;
    }
    validate(compiled, library).expect("compiled graph invariant violated");
}
