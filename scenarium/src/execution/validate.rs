//! Self-consistency checks for the compiled program, runtime cache, and per-run
//! plan. Each fallible `validate` method has an `is_debug()`-gated `validate_debug`
//! wrapper, so production call sites pay nothing while tests can inspect exact
//! validation errors.

use common::is_debug;
use thiserror::Error;

use crate::execution::cache::runtime::RuntimeCache;
use crate::execution::cache::slot::StateOwner;
use crate::execution::compile::CompiledGraph;
use crate::execution::identity::{ExecutionNodeId, FlattenMapValidationError};
use crate::execution::plan::{ExecutionPlan, NodeVerdict};
use crate::execution::program::index::{NodeIdx, NodeSet, OutputAddr};
use crate::execution::program::{ExecutionBinding, ExecutionProgram};
use crate::library::Library;
use crate::node::definition::FuncId;

#[derive(Debug, Error)]
pub(crate) enum CompiledGraphValidationError {
    #[error(transparent)]
    FlattenMap(#[from] FlattenMapValidationError),
    #[error("execution node {e_node_id:?} has a nil func id")]
    NilFuncId { e_node_id: ExecutionNodeId },
    #[error("execution node {e_node_id:?} references missing func {func_id:?}")]
    MissingFunc {
        e_node_id: ExecutionNodeId,
        func_id: FuncId,
    },
    #[error("execution node {e_node_id:?} input arity does not match its function")]
    InputArity { e_node_id: ExecutionNodeId },
    #[error("execution node {e_node_id:?} output arity does not match its function")]
    OutputArity { e_node_id: ExecutionNodeId },
    #[error("execution node {e_node_id:?} event arity does not match its function")]
    EventArity { e_node_id: ExecutionNodeId },
    #[error("execution node {e_node_id:?} input range is out of bounds")]
    InputRange { e_node_id: ExecutionNodeId },
    #[error("execution node {e_node_id:?} output range is out of bounds")]
    OutputRange { e_node_id: ExecutionNodeId },
    #[error("execution node {e_node_id:?} event range is out of bounds")]
    EventRange { e_node_id: ExecutionNodeId },
    #[error("execution node {e_node_id:?} binds to missing output {target:?}")]
    MissingBindingTarget {
        e_node_id: ExecutionNodeId,
        target: OutputAddr,
    },
    #[error("execution node {e_node_id:?} binds to out-of-range output {target:?}")]
    BindingOutputOutOfRange {
        e_node_id: ExecutionNodeId,
        target: OutputAddr,
    },
}

#[derive(Debug, Error)]
pub(crate) enum InstalledGraphValidationError {
    #[error("runtime cache node set does not match the compiled program")]
    NodeSet,
    #[error("runtime cache slot {node_idx:?} holds node {cached:?}, not {e_node_id:?}")]
    NodeMismatch {
        node_idx: NodeIdx,
        cached: ExecutionNodeId,
        e_node_id: ExecutionNodeId,
    },
    #[error("runtime cache output arity does not match node {e_node_id:?}")]
    OutputArity { e_node_id: ExecutionNodeId },
    #[error("runtime cache state owner does not match node {e_node_id:?}")]
    StateOwner { e_node_id: ExecutionNodeId },
}

#[derive(Debug, Error)]
pub(crate) enum ExecutionPlanValidationError {
    #[error("execution order contains more entries than the program")]
    OrderTooLong,
    #[error("execution order contains an out-of-range node index: {node_idx:?}")]
    NodeOutOfRange { node_idx: NodeIdx },
    #[error("execution node {e_node_id:?} input range is out of bounds")]
    InputRange { e_node_id: ExecutionNodeId },
    #[error("execution node {e_node_id:?} appears before dependency {dependency:?}")]
    BeforeDependency {
        e_node_id: ExecutionNodeId,
        dependency: ExecutionNodeId,
    },
    #[error("execution node {e_node_id:?} appears more than once")]
    DuplicateNode { e_node_id: ExecutionNodeId },
    #[error("pinned node {e_node_id:?} is not an execution root")]
    PinnedNodeNotRoot { e_node_id: ExecutionNodeId },
    #[error("event source {e_node_id:?} is not an execution root")]
    EventSourceNotRoot { e_node_id: ExecutionNodeId },
}

impl CompiledGraph {
    /// Self-consistency of the freshly compiled artifact against the `Library`
    /// it was compiled from. The source graph is gone after flattening, so this
    /// validates each `e_node` against its func and checks binding integrity.
    /// The debug wrapper runs at compile (where the library is in hand); the
    /// library-free install-side checks live in [`Self::validate_installed`].
    pub(crate) fn validate(&self, library: &Library) -> Result<(), CompiledGraphValidationError> {
        let program = &self.program;
        self.flatten_map
            .validate(program.e_node_ids.iter().copied())?;
        for (e_node_id, e_node) in program.e_node_ids.iter().zip(&program.e_nodes) {
            if e_node.func_id.is_nil() {
                return Err(CompiledGraphValidationError::NilFuncId {
                    e_node_id: *e_node_id,
                });
            }
            // A special node's interface is its hardcoded spec, not a library func.
            let func = match e_node.special {
                Some(special) => special.func(),
                None => library.by_id(e_node.func_id).ok_or(
                    CompiledGraphValidationError::MissingFunc {
                        e_node_id: *e_node_id,
                        func_id: e_node.func_id,
                    },
                )?,
            };
            if e_node.inputs.len as usize != func.inputs.len() {
                return Err(CompiledGraphValidationError::InputArity {
                    e_node_id: *e_node_id,
                });
            }
            if e_node.outputs.len as usize != func.outputs.len() {
                return Err(CompiledGraphValidationError::OutputArity {
                    e_node_id: *e_node_id,
                });
            }
            if e_node.events.len as usize != func.events.len() {
                return Err(CompiledGraphValidationError::EventArity {
                    e_node_id: *e_node_id,
                });
            }
            let inputs = program.inputs.get(e_node.inputs.range()).ok_or(
                CompiledGraphValidationError::InputRange {
                    e_node_id: *e_node_id,
                },
            )?;
            if program.outputs.get(e_node.outputs.range()).is_none() {
                return Err(CompiledGraphValidationError::OutputRange {
                    e_node_id: *e_node_id,
                });
            }
            if program.events.get(e_node.events.range()).is_none() {
                return Err(CompiledGraphValidationError::EventRange {
                    e_node_id: *e_node_id,
                });
            }

            for e_input in inputs {
                if let ExecutionBinding::Bind(e_addr) = &e_input.binding {
                    let target_e_node = program.e_nodes.get(e_addr.node_idx.idx()).ok_or(
                        CompiledGraphValidationError::MissingBindingTarget {
                            e_node_id: *e_node_id,
                            target: *e_addr,
                        },
                    )?;
                    if e_addr.port_idx >= target_e_node.outputs.len {
                        return Err(CompiledGraphValidationError::BindingOutputOutOfRange {
                            e_node_id: *e_node_id,
                            target: *e_addr,
                        });
                    }
                }
            }
        }
        Ok(())
    }

    /// Debug-only assert form of [`Self::validate`].
    pub(crate) fn validate_debug(&self, library: &Library) {
        if !is_debug() {
            return;
        }
        self.validate(library)
            .expect("compiled graph invariant violated");
    }

    /// The engine's runtime `cache` is index-aligned to exactly this artifact
    /// after `reconcile` — the install-side half of the checks;
    /// artifact-vs-library consistency runs at compile ([`Self::validate`]).
    pub(crate) fn validate_installed(
        &self,
        cache: &RuntimeCache,
    ) -> Result<(), InstalledGraphValidationError> {
        if cache.slots.len() != self.program.e_nodes.len() {
            return Err(InstalledGraphValidationError::NodeSet);
        }

        for (i, ((e_node_id, e_node), (cached, slot))) in self
            .program
            .e_node_ids
            .iter()
            .zip(&self.program.e_nodes)
            .zip(cache.e_node_ids.iter().zip(cache.slots.iter()))
            .enumerate()
        {
            if cached != e_node_id {
                return Err(InstalledGraphValidationError::NodeMismatch {
                    node_idx: NodeIdx(i as u32),
                    cached: *cached,
                    e_node_id: *e_node_id,
                });
            }
            if let Some(output_values) = slot.output_values()
                && output_values.len() != e_node.outputs.len as usize
            {
                return Err(InstalledGraphValidationError::OutputArity {
                    e_node_id: *e_node_id,
                });
            }
            let owner = StateOwner {
                func_id: e_node.func_id,
                version: e_node.version,
            };
            if slot.owner != owner {
                return Err(InstalledGraphValidationError::StateOwner {
                    e_node_id: *e_node_id,
                });
            }
        }
        Ok(())
    }

    /// Debug-only assert form of [`Self::validate_installed`].
    pub(crate) fn validate_installed_debug(&self, cache: &RuntimeCache) {
        if !is_debug() {
            return;
        }
        self.validate_installed(cache)
            .expect("installed compiled graph invariant violated");
    }
}

impl ExecutionPlan {
    /// A planned schedule is a unique post-order DFS whose bindings name valid outputs;
    /// disabled dependencies may remain outside the order.
    pub(crate) fn validate(
        &self,
        program: &ExecutionProgram,
    ) -> Result<(), ExecutionPlanValidationError> {
        if self.process_order.len() > program.e_nodes.len() {
            return Err(ExecutionPlanValidationError::OrderTooLong);
        }

        let mut seen_in_order = NodeSet::default();
        seen_in_order.reset(program.e_nodes.len());
        for &node_idx in &self.process_order {
            let e_node = program
                .e_nodes
                .get(node_idx.idx())
                .ok_or(ExecutionPlanValidationError::NodeOutOfRange { node_idx })?;
            let e_node_id = program.e_node_ids[node_idx];
            let inputs = program
                .inputs
                .get(e_node.inputs.range())
                .ok_or(ExecutionPlanValidationError::InputRange { e_node_id })?;
            for input in inputs {
                if let ExecutionBinding::Bind(addr) = &input.binding {
                    let disabled_dependency = program
                        .e_nodes
                        .get(addr.node_idx.idx())
                        .is_some_and(|e_node| e_node.disabled)
                        && self
                            .verdicts
                            .get(addr.node_idx)
                            .is_some_and(|verdict| *verdict == NodeVerdict::Disabled);
                    if !seen_in_order.contains(addr.node_idx) && !disabled_dependency {
                        return Err(ExecutionPlanValidationError::BeforeDependency {
                            e_node_id,
                            dependency: program.e_node_ids[addr.node_idx],
                        });
                    }
                }
            }
            if seen_in_order.contains(node_idx) {
                return Err(ExecutionPlanValidationError::DuplicateNode { e_node_id });
            }
            seen_in_order.insert(node_idx);
        }

        for node_idx in self.pinned.iter() {
            if !self.roots.contains(node_idx) {
                return Err(ExecutionPlanValidationError::PinnedNodeNotRoot {
                    e_node_id: program.e_node_ids[node_idx],
                });
            }
        }
        for node_idx in self.event_sources.iter() {
            if !self.roots.contains(node_idx) {
                return Err(ExecutionPlanValidationError::EventSourceNotRoot {
                    e_node_id: program.e_node_ids[node_idx],
                });
            }
        }

        Ok(())
    }

    /// Debug-only assert form of [`Self::validate`].
    pub(crate) fn validate_debug(&self, program: &ExecutionProgram) {
        if !is_debug() {
            return;
        }
        self.validate(program)
            .expect("execution plan invariant violated");
    }
}
