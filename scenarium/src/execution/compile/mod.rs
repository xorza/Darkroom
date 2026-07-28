//! Phase 1 of the pipeline, split off the engine so hosts compile on their own
//! thread: validate + flatten the authoring [`Graph`] against the [`Library`]
//! into a self-contained [`CompiledGraph`] the worker installs as-is. Compile
//! errors surface synchronously at the call site — a graph that doesn't
//! compile is never sent, so the worker's install is infallible and a running
//! event loop is never disturbed by a bad edit.

use hashbrown::HashMap;
use thiserror::Error;

use crate::execution::flatten::Flattener;
use crate::execution::flatten::map::FlattenMap;
use crate::execution::identity::{ExecutionIdentityError, ExecutionNodeId};
use crate::execution::program::index::{NodeIdx, NodeSet};
use crate::execution::program::{ExecutionBinding, ExecutionNode, ExecutionProgram};
use crate::graph::{Graph, NodeId};
use crate::library::Library;
use crate::node::definition::FuncBehavior;

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
    /// Authored node → every execution node covering it, ascending.
    ///
    /// The inverse of [`FlattenMap::attribution`], and the direction every
    /// question a host asks runs in: what does running this node mean, what
    /// does evicting it reach, is it a sink. Attribution answers one node at a
    /// time, so deriving this on demand costs a walk of the whole program per
    /// question — and the editor asks two per node per frame. Inverting once
    /// here is the same single pass, paid at compile.
    footprints: HashMap<NodeId, Vec<NodeIdx>>,
    /// Data edges reversed: which nodes read each node's outputs. A pure
    /// function of the program, so it is built with the program rather than
    /// rebuilt by each caller that needs it.
    consumers: HashMap<NodeIdx, Vec<NodeIdx>>,
    /// Graph instance → the execution nodes behind its exposed output
    /// ports, resolved from [`FlattenMap::exposed_producers`].
    ///
    /// Not derivable from `consumers`: flattening removes the
    /// `GraphOutput` edges, so this is the only surviving record of which
    /// interior nodes an instance exists to produce.
    exposed: HashMap<NodeId, Vec<NodeIdx>>,
}

impl CompiledGraph {
    /// Pair a program with its flatten map and index the two relations every
    /// query needs. The only way to build one — the indices are not optional
    /// state, and nothing may observe a `CompiledGraph` without them.
    fn indexed(program: ExecutionProgram, flatten_map: FlattenMap) -> Self {
        let mut footprints: HashMap<NodeId, Vec<NodeIdx>> = HashMap::new();
        // Ascending index order, so every footprint lands sorted and
        // `run_targets` can binary-search it.
        for (node_idx, e_node_id) in program.e_node_ids.iter_indexed() {
            let attribution = flatten_map
                .attribution(*e_node_id)
                .expect("every execution node has authored attribution");
            for authored in attribution {
                footprints.entry(authored).or_default().push(node_idx);
            }
        }
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
        let mut exposed: HashMap<NodeId, Vec<NodeIdx>> = HashMap::new();
        for (instance, producer) in flatten_map.exposed_producers() {
            if let Some(&node_idx) = program.e_node_index.get(&producer) {
                exposed.entry(instance).or_default().push(node_idx);
            }
        }
        Self {
            program,
            flatten_map,
            footprints,
            consumers,
            exposed,
        }
    }

    /// Every execution node an authored node covers — its *footprint* —
    /// ascending, empty when it covers no compiled work.
    ///
    /// A leaf in the entry graph covers itself; a leaf inside a definition
    /// covers one occurrence per instance of that definition; a graph instance
    /// covers its whole flattened interior. This is the only way from an
    /// authored id to execution ids: a composite dissolves at flatten time and
    /// has no id of its own, so *deriving* one
    /// ([`ExecutionNodeId::from_authoring`]) answers for a top-level leaf and
    /// nothing else.
    fn footprint(&self, node_id: NodeId) -> &[NodeIdx] {
        self.footprints.get(&node_id).map_or(&[][..], Vec::as_slice)
    }

    /// The nodes reading `node_idx`'s outputs. Empty when nothing does.
    fn consumers_of(&self, node_idx: NodeIdx) -> &[NodeIdx] {
        self.consumers.get(&node_idx).map_or(&[][..], Vec::as_slice)
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

    /// Whether an authored node performs sink work — runs for its effect
    /// rather than for a value some consumer reads.
    ///
    /// A func is one when its declaration says so. A graph instance is one
    /// when anything inside it is: a sinks run reaches that interior sink
    /// either way, and disabling or subscribing the instance is what governs
    /// it. Having outputs of its own does not stop a composite being a sink,
    /// the way a portless func signals it.
    pub fn is_sink(&self, node_id: NodeId) -> Option<bool> {
        self.any_occurrence(node_id, |e_node| e_node.sink)
    }

    /// Whether an authored node holds work that recomputes every run.
    ///
    /// An impure node has no content digest, so nothing keys a cache on it.
    /// A graph instance inherits that from its interior: one impure node in
    /// there is enough for the instance to stop being reusable as a whole,
    /// even though its pure upstream still caches.
    pub fn is_impure(&self, node_id: NodeId) -> Option<bool> {
        self.any_occurrence(node_id, |e_node| e_node.behavior == FuncBehavior::Impure)
    }

    /// Fold a per-node fact over an authored node's footprint: whether any
    /// occurrence satisfies `holds`.
    ///
    /// `None` when the node covers no compiled work — a boundary node, a
    /// definition no instance reaches, or a program that hasn't been built
    /// yet. There is nothing to fold, so the caller keeps whatever it can
    /// derive from the authoring graph alone.
    fn any_occurrence(
        &self,
        node_id: NodeId,
        holds: impl Fn(&ExecutionNode) -> bool,
    ) -> Option<bool> {
        let footprint = self.footprint(node_id);
        if footprint.is_empty() {
            return None;
        }
        Some(
            footprint
                .iter()
                .any(|&node_idx| holds(&self.program.e_nodes[node_idx])),
        )
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
        let footprint = self.footprint(node_id);
        // Footprints are built in index order, so membership is a search.
        let inside = |node_idx: &NodeIdx| footprint.binary_search(node_idx).is_ok();
        // What the instance exposes, taken from the record flatten kept
        // rather than inferred. "Its value leaves the footprint" is not
        // observable in the finished program: the `GraphOutput` edge that
        // carried it is gone, so an exposed producer that an interior node
        // also reads looked purely internal and dropped out of the seeds —
        // while a dead interior terminal, with no readers at all, stayed
        // in. The request then ran the wrong cone entirely.
        let exposed = self.exposed.get(&node_id).map_or(&[][..], Vec::as_slice);
        footprint
            .iter()
            .filter(|&&node_idx| {
                let readers = self.consumers_of(node_idx);
                self.program.e_nodes[node_idx].sink
                    || exposed.contains(&node_idx)
                    || readers.is_empty()
                    || !readers.iter().all(inside)
            })
            .map(|&node_idx| self.program.e_node_ids[node_idx])
            .collect()
    }

    /// Resolve authored nodes or graph instances to their flattened occurrences,
    /// then return their reflexive transitive closure over data-consumer edges.
    pub(crate) fn data_consumer_closure(
        &self,
        authored_node_ids: &[NodeId],
    ) -> Vec<ExecutionNodeId> {
        let mut in_closure = NodeSet::default();
        in_closure.reset(self.program.e_nodes.len());
        let mut pending: Vec<NodeIdx> = authored_node_ids
            .iter()
            .flat_map(|node_id| self.footprint(*node_id))
            .copied()
            .filter(|&node_idx| {
                // Two authored ids can name overlapping footprints — an
                // instance and something inside it — so the seeds dedup too.
                let fresh = !in_closure.contains(node_idx);
                in_closure.insert(node_idx);
                fresh
            })
            .collect();
        while let Some(node_idx) = pending.pop() {
            for &consumer_idx in self.consumers_of(node_idx) {
                if !in_closure.contains(consumer_idx) {
                    in_closure.insert(consumer_idx);
                    pending.push(consumer_idx);
                }
            }
        }

        let closure: Vec<ExecutionNodeId> = in_closure
            .iter()
            .map(|node_idx| self.program.e_node_ids[node_idx])
            .collect();
        debug_assert!(
            closure.is_sorted(),
            "dense indices are assigned in id order, so an ascending index walk yields ascending ids"
        );
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

        let compiled = CompiledGraph::indexed(program, flatten_map);
        compiled.validate_debug(library);
        Ok(compiled)
    }
}

#[cfg(any(test, feature = "internals"))]
pub(crate) mod internals {
    use std::sync::Arc;

    use crate::execution::compile::CompiledGraph;
    use crate::execution::flatten::map::FlattenMap;
    use crate::execution::identity::ExecutionNodeId;
    use crate::execution::program::ExecutionProgram;
    use crate::graph::NodeId;

    impl CompiledGraph {
        /// Every execution node an authored node covers, in ascending id
        /// order — [`CompiledGraph::footprint`] spelled out.
        ///
        /// Production never needs the set itself, only the questions asked of
        /// it (`run_targets`, `is_sink`, `is_impure`, `data_consumer_closure`),
        /// so this exists to test the relation those four share once rather
        /// than four times through their filters.
        pub fn occurrences(&self, node_id: NodeId) -> Vec<ExecutionNodeId> {
            self.footprint(node_id)
                .iter()
                .map(|&node_idx| self.program.e_node_ids[node_idx])
                .collect()
        }
    }

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
            Arc::new(CompiledGraph::indexed(
                ExecutionProgram::default(),
                self.flatten_map,
            ))
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
