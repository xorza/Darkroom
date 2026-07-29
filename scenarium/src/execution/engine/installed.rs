//! The installed compile artifact and the runtime cache aligned to it.
//!
//! Replacing the artifact and reconciling the cache are one operation on this
//! owner, so the two cannot be updated independently. The alignment checks live
//! here for the same reason: they are invariants of the installed pair, not of
//! either component alone.

use std::sync::Arc;

use ::common::is_debug;
use thiserror::Error;

use crate::execution::cache::runtime::RuntimeCache;
use crate::execution::cache::slot::StateOwner;
use crate::execution::compiled::CompiledGraph;
use crate::execution::identity::ExecutionNodeId;

#[derive(Debug, Default)]
pub(super) struct InstalledGraph {
    pub(super) compiled: Option<Arc<CompiledGraph>>,
    pub(super) cache: RuntimeCache,
}

impl InstalledGraph {
    pub(super) fn is_empty(&self) -> bool {
        self.compiled
            .as_deref()
            .is_none_or(|compiled| compiled.program.e_nodes.is_empty())
    }

    pub(super) fn clear(&mut self) {
        self.compiled = None;
        self.cache.clear();
    }

    /// Replace the immutable artifact and reconcile the cache onto its dense
    /// node space as one operation.
    pub(super) fn replace(&mut self, compiled: Arc<CompiledGraph>) {
        self.cache.reconcile(&compiled);
        self.compiled = Some(compiled);
        self.validate_debug();
    }

    /// Self-consistency of the installed artifact/cache pair.
    fn validate(&self) -> Result<(), InstalledGraphValidationError> {
        let compiled = self
            .compiled
            .as_ref()
            .expect("validation requires an installed compiled graph");
        let program = &compiled.program;
        if self.cache.slot_count() != program.e_nodes.len() {
            return Err(InstalledGraphValidationError::NodeCount {
                slots: self.cache.slot_count(),
                expected: program.e_nodes.len(),
            });
        }
        if !self.cache.is_aligned_to(compiled) {
            return Err(InstalledGraphValidationError::ArtifactMismatch);
        }

        for ((e_node_id, e_node), slot) in program
            .e_node_ids
            .iter()
            .zip(program.e_nodes.iter())
            .zip(self.cache.slots())
        {
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

    fn validate_debug(&self) {
        if !is_debug() {
            return;
        }
        self.validate()
            .expect("installed compiled graph invariant violated");
    }
}

#[derive(Debug, Error)]
enum InstalledGraphValidationError {
    #[error("runtime cache spans {slots} nodes, not the compiled program's {expected}")]
    NodeCount { slots: usize, expected: usize },
    #[error("runtime cache is not aligned to the installed compiled artifact")]
    ArtifactMismatch,
    #[error("runtime cache output arity does not match node {e_node_id:?}")]
    OutputArity { e_node_id: ExecutionNodeId },
    #[error("runtime cache state owner does not match node {e_node_id:?}")]
    StateOwner { e_node_id: ExecutionNodeId },
}

#[cfg(test)]
mod internals {
    use crate::execution::compiled::CompiledGraph;
    use crate::execution::engine::installed::InstalledGraph;

    impl InstalledGraph {
        /// The installed artifact itself, for the engine's test-only
        /// introspection. Production reaches the pair through the methods
        /// above, which is what keeps the artifact and its cache moving
        /// together.
        pub(crate) fn compiled(&self) -> &CompiledGraph {
            self.compiled
                .as_deref()
                .expect("execution requires an installed compiled graph")
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::execution::compile::internals::CompiledGraphBuilder;
    use crate::execution::engine::installed::InstalledGraph;
    use crate::execution::identity::ExecutionNodeId;
    use crate::graph::identity::NodeId;

    #[test]
    fn replacement_installs_one_canonical_artifact_for_engine_and_cache() {
        let mut builder = CompiledGraphBuilder::new();
        builder.insert_leaf(ExecutionNodeId::unique(), [], NodeId::unique());
        let compiled = builder.build();
        let mut installed = InstalledGraph::default();

        installed.replace(Arc::clone(&compiled));

        assert!(Arc::ptr_eq(installed.compiled.as_ref().unwrap(), &compiled));
        assert!(installed.cache.is_aligned_to(&compiled));
        installed.validate().unwrap();
    }

    #[test]
    fn validation_rejects_a_cache_with_the_wrong_node_count() {
        let mut builder = CompiledGraphBuilder::new();
        builder.insert_leaf(ExecutionNodeId::unique(), [], NodeId::unique());
        let installed = InstalledGraph {
            compiled: Some(builder.build()),
            ..Default::default()
        };

        assert_eq!(
            installed.validate().unwrap_err().to_string(),
            "runtime cache spans 0 nodes, not the compiled program's 1"
        );
    }
}
