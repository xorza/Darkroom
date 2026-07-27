//! Phase 1 of the pipeline, split off the engine so hosts compile on their own
//! thread: validate + flatten the authoring [`Graph`] against the [`Library`]
//! into a self-contained [`CompiledGraph`] the worker installs as-is. Compile
//! errors surface synchronously at the call site — a graph that doesn't
//! compile is never sent, so the worker's install is infallible and a running
//! event loop is never disturbed by a bad edit.

use hashbrown::{HashMap, HashSet};
use thiserror::Error;

use crate::execution::flatten::Flattener;
use crate::execution::identity::{ExecutionIdentityError, ExecutionNodeId, FlattenMap};
use crate::execution::program::index::{NodeIdx, NodeSet};
use crate::execution::program::{ExecutionBinding, ExecutionProgram};
use crate::graph::{Graph, NodeId};
use crate::library::Library;

/// The graph won't compile against the library: a document can be stale
/// against an evolved library (a dropped func, a shrunk port list, a
/// type-mismatched binding), so this is a recoverable error the caller
/// surfaces, not a logic bug. The compile-phase counterpart of the run-phase
/// [`Error`](crate::execution::error::Error) — the two can't be confused at the type
/// level, and only `compile` produces it.
#[derive(Debug, Error)]
#[error("invalid graph: {message}")]
pub struct CompileError {
    pub message: String,
}

/// The compile artifact: the flattened, immutable program (lambdas, resolved
/// output types, and bound-path stamping metadata) plus the [`FlattenMap`] that
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

    /// Every execution node an authored node covers — its *footprint*.
    ///
    /// A leaf in the entry graph covers itself; a leaf inside a definition
    /// covers one occurrence per instance of that definition; a graph
    /// instance covers its whole flattened interior. The inverse of
    /// [`Self::attribution`], and the only supported way to go from an
    /// authored id to execution ids: a composite dissolves at flatten time
    /// and has no execution id of its own, so *deriving* one
    /// ([`ExecutionNodeId::from_authoring`]) answers only for a top-level
    /// leaf, while this answers for every authored node.
    ///
    /// Ascending id order, like [`Self::data_consumer_closure`].
    pub fn occurrences(&self, node_id: NodeId) -> Vec<ExecutionNodeId> {
        let program = &self.program;
        self.footprint(|covered| covered == node_id)
            .iter()
            .map(|node_idx| program.e_node_ids[node_idx])
            .collect()
    }

    /// The execution nodes a "run this node" seeds: those producing what the
    /// node exposes, plus any sink it contains.
    ///
    /// Stated without naming a node kind — an occurrence qualifies when it
    /// is a sink, or when its value leaves the footprint (something outside
    /// consumes it, or nothing does). For a leaf that is the node itself,
    /// exactly as before; for a graph instance it is the interior producers
    /// behind its output ports plus its interior sinks, and *not* the
    /// interior wiring between them — that still runs, as their upstream
    /// cone.
    ///
    /// Empty when the node has no footprint at all: a boundary node, or one
    /// absent from this program.
    pub fn run_targets(&self, node_id: NodeId) -> Vec<ExecutionNodeId> {
        let program = &self.program;
        let footprint = self.footprint(|covered| covered == node_id);
        let consumers = self.consumers();
        footprint
            .iter()
            .filter(|&node_idx| {
                program.e_nodes[node_idx].sink
                    || consumers
                        .get(&node_idx)
                        .is_none_or(|of| of.iter().any(|idx| !footprint.contains(*idx)))
            })
            .map(|node_idx| program.e_node_ids[node_idx])
            .collect()
    }

    /// Resolve authored nodes or graph instances to their flattened occurrences,
    /// then return their reflexive transitive closure over data-consumer edges.
    pub(crate) fn data_consumer_closure(
        &self,
        authored_node_ids: &[NodeId],
    ) -> Vec<ExecutionNodeId> {
        let program = &self.program;
        let selected: HashSet<NodeId> = authored_node_ids.iter().copied().collect();
        let mut in_closure = self.footprint(|covered| selected.contains(&covered));
        let mut pending: Vec<NodeIdx> = in_closure.iter().collect();
        let consumers = self.consumers();
        while let Some(node_idx) = pending.pop() {
            for &consumer_idx in consumers.get(&node_idx).into_iter().flatten() {
                if !in_closure.contains(consumer_idx) {
                    in_closure.insert(consumer_idx);
                    pending.push(consumer_idx);
                }
            }
        }

        let closure: Vec<ExecutionNodeId> = in_closure
            .iter()
            .map(|node_idx| program.e_node_ids[node_idx])
            .collect();
        debug_assert!(
            closure.is_sorted(),
            "dense indices are assigned in id order, so an ascending index walk yields ascending ids"
        );
        closure
    }

    /// Every execution node whose attribution names an authored node `covers`
    /// accepts — the shared walk behind [`Self::occurrences`],
    /// [`Self::run_targets`], and [`Self::data_consumer_closure`].
    ///
    /// Attribution yields the authored leaf followed by each enclosing
    /// instance, so testing every element is what makes a graph instance
    /// match its whole interior without the caller naming a node kind.
    fn footprint(&self, covers: impl Fn(NodeId) -> bool) -> NodeSet {
        let program = &self.program;
        let mut footprint = NodeSet::default();
        footprint.reset(program.e_nodes.len());
        for (node_idx, e_node_id) in program.e_node_ids.iter_indexed() {
            if self
                .flatten_map
                .attribution(*e_node_id)
                .expect("every execution node has authored attribution")
                .any(&covers)
            {
                footprint.insert(node_idx);
            }
        }
        footprint
    }

    /// Data-consumer edges, reversed: which nodes read each node's outputs.
    fn consumers(&self) -> HashMap<NodeIdx, Vec<NodeIdx>> {
        let program = &self.program;
        let mut consumers: HashMap<NodeIdx, Vec<NodeIdx>> = HashMap::new();
        for (node_idx, e_node) in program.e_nodes.iter_indexed() {
            for input in &program.inputs[e_node.inputs] {
                if let ExecutionBinding::Bind(address) = &input.binding {
                    consumers
                        .entry(address.node_idx)
                        .or_default()
                        .push(node_idx);
                }
            }
        }
        consumers
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
    /// [installed](crate::execution::engine::ExecutionEngine::install) into an engine
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
        self.flattener
            .build(&mut program, graph, library, &mut flatten_map);

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
pub(crate) mod internals {
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
mod tests;
