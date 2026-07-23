//! Phase 1 of the pipeline, split off the engine so hosts compile on their own
//! thread: validate + flatten the authoring [`Graph`] against the [`Library`]
//! into a self-contained [`CompiledGraph`] the worker installs as-is. Compile
//! errors surface synchronously at the call site — a graph that doesn't
//! compile is never sent, so the worker's install is infallible and a running
//! event loop is never disturbed by a bad edit.

use hashbrown::{HashMap, HashSet};
use thiserror::Error;

use crate::execution::flatten::{Flattener, Pools};
use crate::execution::identity::{ExecutionIdentityError, ExecutionNodeId, FlattenMap};
use crate::execution::program::{ExecutionBinding, ExecutionProgram};
use crate::graph::{Graph, NodeId};
use crate::library::Library;

/// The graph won't compile against the library: a document can be stale
/// against an evolved library (a dropped func, a shrunk port list, a
/// type-mismatched binding), so this is a recoverable error the caller
/// surfaces, not a logic bug. The compile-phase counterpart of the run-phase
/// [`Error`](crate::execution::Error) — the two can't be confused at the type
/// level, and only `compile` produces it.
#[derive(Debug, Error)]
#[error("invalid graph: {message}")]
pub struct CompileError {
    pub message: String,
}

/// The compile artifact: the flattened, immutable program (lambdas, resolved
/// output types, and input stampers attached) plus the [`FlattenMap`] that
/// attributes execution identities to authored nodes. Self-contained — executing
/// it needs neither the authoring graph nor the library. `Default` is the empty
/// program (the engine's pre-install / cleared state).
#[derive(Debug, Default)]
pub struct CompiledGraph {
    pub(crate) program: ExecutionProgram,
    pub(crate) flatten_map: FlattenMap,
}

impl CompiledGraph {
    /// Return the authored leaf node that produced one execution node.
    pub fn leaf(&self, e_node_id: ExecutionNodeId) -> Result<NodeId, ExecutionIdentityError> {
        Ok(self
            .attribution(e_node_id)?
            .next()
            .expect("execution attribution must start with its authored leaf"))
    }

    /// Attribute one flat execution id to its authored node followed by every
    /// enclosing graph instance, innermost first.
    pub fn attribution(
        &self,
        e_node_id: ExecutionNodeId,
    ) -> Result<impl Iterator<Item = NodeId> + '_, ExecutionIdentityError> {
        self.flatten_map
            .attribution(e_node_id)
            .ok_or(ExecutionIdentityError::NodeNotFound { e_node_id })
    }

    /// Resolve authored nodes or graph instances to their flattened occurrences,
    /// then return their reflexive transitive closure over data-consumer edges.
    pub(crate) fn data_consumer_closure(
        &self,
        authored_node_ids: &[NodeId],
    ) -> Vec<ExecutionNodeId> {
        let selected: HashSet<NodeId> = authored_node_ids.iter().copied().collect();
        let mut closure: HashSet<ExecutionNodeId> = self
            .program
            .e_nodes
            .keys()
            .copied()
            .filter(|e_node_id| {
                self.flatten_map
                    .attribution(*e_node_id)
                    .expect("every execution node has authored attribution")
                    .any(|node_id| selected.contains(&node_id))
            })
            .collect();

        let mut consumers: HashMap<ExecutionNodeId, Vec<ExecutionNodeId>> = HashMap::new();
        for (consumer_id, e_node) in &self.program.e_nodes {
            for input in self.program.node_inputs(e_node) {
                if let ExecutionBinding::Bind(address) = &input.binding {
                    consumers
                        .entry(address.e_node_id)
                        .or_default()
                        .push(*consumer_id);
                }
            }
        }
        let mut pending: Vec<_> = closure.iter().copied().collect();
        while let Some(e_node_id) = pending.pop() {
            for consumer_id in consumers.get(&e_node_id).into_iter().flatten() {
                if closure.insert(*consumer_id) {
                    pending.push(*consumer_id);
                }
            }
        }

        let mut closure: Vec<_> = closure.into_iter().collect();
        closure.sort_unstable();
        closure
    }
}

/// The compile entry point, owning reusable [`Flattener`] traversal scratch.
/// Hosts keep one per compile site (e.g. darkroom's `Engine`); the produced
/// [`CompiledGraph`] is always fresh and can be shared with the worker in an
/// [`Arc`](std::sync::Arc).
#[derive(Debug, Default)]
pub struct Compiler {
    flattener: Flattener,
}

impl Compiler {
    /// Compile `graph` against `library`: validate, flatten composites into a
    /// flat func-only program, and resolve the output-type pool. Pure CPU on
    /// the caller's thread; the result is
    /// [installed](crate::execution::ExecutionEngine::install) into an engine
    /// (typically across the worker channel).
    pub fn compile(
        &mut self,
        graph: &Graph,
        library: &Library,
    ) -> Result<CompiledGraph, CompileError> {
        // Validate before building anything: the graph+library pair is untrusted
        // input, and a passing check lets the flatten pass resolve every
        // reference infallibly.
        if let Err(e) = graph.validate_for_execution(library) {
            tracing::error!(error = %e, "compile rejected: invalid graph");
            return Err(CompileError {
                message: e.to_string(),
            });
        }

        // Flatten graphs straight into execution nodes — no intermediate
        // `Graph`. Everything downstream is boundary-agnostic (func nodes only).
        let mut program = ExecutionProgram::default();
        let mut flatten_map = FlattenMap::default();
        self.flattener.build(
            &mut program.e_nodes,
            Pools {
                inputs: &mut program.inputs,
                events: &mut program.events,
                output_pinned: &mut program.output_pinned,
            },
            graph,
            library,
            &mut flatten_map,
        );

        // Resolve types here so runtime digesting does not retain the function library.
        program.resolve_output_types(library);

        let compiled = CompiledGraph {
            program,
            flatten_map,
        };
        compiled.validate_debug(library);
        Ok(compiled)
    }
}

#[cfg(any(test, feature = "internals"))]
pub(crate) mod test_support {
    use std::sync::Arc;

    use crate::execution::compile::CompiledGraph;
    use crate::execution::identity::{ExecutionNodeId, FlattenMap};
    use crate::execution::program::ExecutionProgram;
    use crate::graph::NodeId;

    #[derive(Debug)]
    pub struct CompiledGraphBuilder {
        flatten_map: FlattenMap,
    }

    impl CompiledGraphBuilder {
        pub fn new() -> Self {
            let mut flatten_map = FlattenMap::default();
            flatten_map.reset();
            Self { flatten_map }
        }

        pub fn insert_leaf(
            &mut self,
            e_node_id: ExecutionNodeId,
            instances: impl IntoIterator<Item = NodeId>,
            node_id: NodeId,
        ) {
            let mut scope = 0;
            for instance in instances {
                scope = self.flatten_map.push_scope(instance, scope);
            }
            self.flatten_map.set_leaf(e_node_id, scope, node_id);
        }

        pub fn build(self) -> Arc<CompiledGraph> {
            Arc::new(CompiledGraph {
                program: ExecutionProgram::default(),
                flatten_map: self.flatten_map,
            })
        }
    }

    impl Default for CompiledGraphBuilder {
        fn default() -> Self {
            Self::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execution::cache::RuntimeCache;
    use crate::execution::identity::test_support::FlattenMapBuilder;
    use crate::execution::program::ExecutionNode;
    use crate::graph::NodeSearch;
    use crate::graph::interface::{GraphId, GraphLink};
    use crate::node::definition::FuncId;
    use crate::testing::{TestFuncHooks, test_func_lib};

    #[test]
    fn compilation_retains_a_disabled_composite_interior_as_disabled() {
        let library = test_func_lib(TestFuncHooks::default());
        let mut nested = Graph::new("Nested");
        let interior_id = nested.add(library.by_name("Print").unwrap().into());
        let nested_id = GraphId::unique();

        let mut graph = Graph::default();
        let instance_id = graph.add_graph_node(&nested, GraphLink::Local(nested_id));
        graph
            .find_mut(&instance_id, NodeSearch::TopLevel)
            .unwrap()
            .disabled = true;
        graph.insert_graph(nested_id, nested);

        let compiled = Compiler::default().compile(&graph, &library).unwrap();
        let e_node_id = ExecutionNodeId::from_authoring(&[instance_id, interior_id]);
        assert!(
            compiled.program.e_nodes[&e_node_id].disabled,
            "the disabled instance marks its compiled interior effectively disabled"
        );
    }

    #[test]
    fn data_consumer_closure_targets_one_instance_or_every_shared_definition_occurrence() {
        let library = test_func_lib(TestFuncHooks::default());
        let mut nested = Graph::new("Nested");
        let interior_id = nested.add(library.by_name("get_b").unwrap().into());
        let nested_id = GraphId::unique();

        let mut graph = Graph::default();
        let first_instance = graph.add_graph_node(&nested, GraphLink::Local(nested_id));
        let second_instance = graph.add_graph_node(&nested, GraphLink::Local(nested_id));
        graph.insert_graph(nested_id, nested);
        let first = ExecutionNodeId::from_authoring(&[first_instance, interior_id]);
        let second = ExecutionNodeId::from_authoring(&[second_instance, interior_id]);
        let compiled = Compiler::default().compile(&graph, &library).unwrap();

        assert_eq!(
            compiled.data_consumer_closure(&[first_instance]),
            vec![first],
            "an instance evicts only its own flattened interior"
        );
        let mut both = vec![first, second];
        both.sort_unstable();
        assert_eq!(
            compiled.data_consumer_closure(&[interior_id]),
            both,
            "editing a shared definition evicts every flattened occurrence"
        );
    }

    #[test]
    fn validation_returns_compiled_and_installed_mismatches() {
        let e_node_id = ExecutionNodeId::unique();
        let interior = NodeId::unique();
        let missing_func = FuncId::unique();
        let mut builder = FlattenMapBuilder::new();
        builder.insert_leaf(e_node_id, [], interior);
        let mut program = ExecutionProgram::default();
        program.e_nodes.insert(
            e_node_id,
            ExecutionNode {
                func_id: missing_func,
                ..Default::default()
            },
        );
        let compiled = CompiledGraph {
            program,
            flatten_map: builder.build(),
        };

        assert_eq!(
            compiled
                .validate(&Library::default())
                .unwrap_err()
                .to_string(),
            format!("execution node {e_node_id:?} references missing func {missing_func:?}")
        );
        assert_eq!(
            compiled
                .validate_installed(&RuntimeCache::default())
                .unwrap_err()
                .to_string(),
            "runtime cache node set does not match the compiled program"
        );

        assert_eq!(
            compiled.attribution(e_node_id).unwrap().collect::<Vec<_>>(),
            vec![interior]
        );
        assert_eq!(compiled.leaf(e_node_id).unwrap(), interior);

        let missing_node = ExecutionNodeId::unique();
        assert_eq!(
            compiled.leaf(missing_node),
            Err(ExecutionIdentityError::NodeNotFound {
                e_node_id: missing_node,
            })
        );
        assert!(matches!(
            compiled.attribution(missing_node),
            Err(ExecutionIdentityError::NodeNotFound { e_node_id }) if e_node_id == missing_node
        ));
    }
}
