//! Phase 1 of the pipeline, split off the engine so hosts compile on their own
//! thread: validate + flatten the authoring [`Graph`] against the [`Library`]
//! into a self-contained [`CompiledGraph`] the worker installs as-is. Compile
//! errors surface synchronously at the call site — a graph that doesn't
//! compile is never sent, so the worker's install is infallible and a running
//! event loop is never disturbed by a bad edit.

use std::sync::Arc;

use common::is_debug;
use hashbrown::HashMap;
use thiserror::Error;

use crate::execution::cache::runtime::RuntimeCache;
use crate::execution::cache::slot::StateOwner;
use crate::execution::flatten::Flattener;
use crate::execution::flatten::map::{FlattenMap, FlattenMapValidationError};
use crate::execution::identity::{ExecutionIdentityError, ExecutionNodeId};
use crate::execution::program::index::{NodeColumn, NodeIdx, NodeSet, OutputAddr};
use crate::execution::program::pool::{Pool, PoolRange};
use crate::execution::program::{ExecutionBinding, Program};
use crate::graph::Graph;
use crate::graph::address::NodeId;
use crate::library::Library;
use crate::node::definition::{FuncBehavior, FuncId};

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

/// Self-consistency checks for the compile artifact. Each fallible `validate` has an
/// `is_debug()`-gated `validate_debug` wrapper, so production call sites pay nothing
/// while tests can inspect exact validation errors.
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
    #[error(
        "execution node {e_node_id:?} has an event subscriber outside the program: {subscriber:?}"
    )]
    MissingEventSubscriber {
        e_node_id: ExecutionNodeId,
        subscriber: NodeIdx,
    },
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
    #[error("runtime cache output arity does not match node {e_node_id:?}")]
    OutputArity { e_node_id: ExecutionNodeId },
    #[error("runtime cache state owner does not match node {e_node_id:?}")]
    StateOwner { e_node_id: ExecutionNodeId },
}

/// The compile artifact: the flattened, immutable program (lambdas, resolved
/// output types, and bound-path stamping metadata) plus the `FlattenMap` that
/// attributes execution identities to authored nodes. Self-contained — executing
/// it needs neither the authoring graph nor the library. `Default` is the empty
/// program (the engine's pre-install / cleared state).
#[derive(Debug, Default)]
pub struct CompiledGraph {
    /// Shared rather than owned outright so the [`RuntimeCache`] can hold the
    /// program its slots are aligned to without copying anything to name them
    /// by. One allocation, two holders: the artifact and the cache reconciled
    /// onto it.
    pub(crate) program: Arc<Program>,
    pub(crate) flatten_map: FlattenMap,
    /// The packed backing of all three relations below: each of them owns
    /// runs of this one buffer rather than a `Vec` of its own per key.
    node_lists: Pool<NodeIdx>,
    /// Authored node → every execution node covering it, ascending.
    ///
    /// The inverse of [`FlattenMap::attribution`], and the direction every
    /// question a host asks runs in: what does running this node mean, what
    /// does evicting it reach, is it a sink. Attribution answers one node at a
    /// time, so deriving this on demand costs a walk of the whole program per
    /// question — and the editor asks two per node per frame. Inverting once
    /// here is the same single pass, paid at compile.
    footprints: HashMap<NodeId, PoolRange<NodeIdx>>,
    /// Data edges reversed: which nodes read each node's outputs. A pure
    /// function of the program, so it is built with the program rather than
    /// rebuilt by each caller that needs it.
    ///
    /// A column rather than a map: the key is a dense index, and the two
    /// walks that read it — `run_targets` per footprint node, the eviction
    /// closure per node reached — ask for one node after another.
    consumers: NodeColumn<PoolRange<NodeIdx>>,
    /// Graph instance → the execution nodes behind its exposed output
    /// ports, resolved from [`FlattenMap::exposed_producers`].
    ///
    /// Not derivable from `consumers`: flattening removes the
    /// `GraphOutput` edges, so this is the only surviving record of which
    /// interior nodes an instance exists to produce.
    exposed: HashMap<NodeId, PoolRange<NodeIdx>>,
}

/// Group `entries` by authored node, each group one packed run of `lists`.
///
/// Sorted as pairs, so a group is contiguous *and* ascending by index
/// inside — the order [`CompiledGraph::run_targets`] binary-searches.
fn pack_groups(
    mut entries: Vec<(NodeId, NodeIdx)>,
    lists: &mut Pool<NodeIdx>,
) -> HashMap<NodeId, PoolRange<NodeIdx>> {
    entries.sort_unstable();
    let mut ranges = HashMap::new();
    for group in entries.chunk_by(|left, right| left.0 == right.0) {
        ranges.insert(
            group[0].0,
            lists.append(group.iter().map(|&(_, node_idx)| node_idx)),
        );
    }
    ranges
}

impl CompiledGraph {
    /// Pair a program with its flatten map and index the three relations
    /// every query needs. The only way to build one — the indices are not
    /// optional state, and nothing may observe a `CompiledGraph` without
    /// them.
    ///
    /// Each relation is collected flat, then grouped into one shared
    /// buffer, so all three cost one allocation between them rather than a
    /// `Vec` per authored node and per producer — in a structure the host
    /// holds for as long as the compiled graph lives.
    fn indexed(program: Program, flatten_map: FlattenMap) -> Self {
        let mut node_lists = Pool::default();

        let mut occurrences = Vec::new();
        for (node_idx, e_node_id) in program.e_node_ids.iter_indexed() {
            let attribution = flatten_map
                .attribution(*e_node_id)
                .expect("every execution node has authored attribution");
            occurrences.extend(attribution.map(|node_id| (node_id, node_idx)));
        }
        let footprints = pack_groups(occurrences, &mut node_lists);

        let exposed_producers = flatten_map
            .exposed_producers()
            .map(|(instance, producer)| {
                // Every producer recorded is a node the same walk emitted.
                // Dropping one it could not find would leave the instance
                // without the record `run_targets` exists to read — the
                // silent miss the record was added to end.
                let node_idx = *program
                    .e_node_index
                    .get(&producer)
                    .expect("flatten records exposed producers it emitted");
                (instance, node_idx)
            })
            .collect();
        let exposed = pack_groups(exposed_producers, &mut node_lists);

        let mut edges = Vec::new();
        for (node_idx, e_node) in program.e_nodes.iter_indexed() {
            for input in &program.inputs[e_node.inputs] {
                if let ExecutionBinding::Bind(address) = &input.binding {
                    edges.push((address.node_idx, node_idx));
                }
            }
        }
        // The same grouping as `pack_groups`, into a column: the key is a
        // dense index, so every read is an offset rather than a hash.
        edges.sort_unstable();
        let mut consumers = NodeColumn::default();
        consumers.reset(program.e_nodes.len(), PoolRange::default());
        for group in edges.chunk_by(|left, right| left.0 == right.0) {
            consumers[group[0].0] = node_lists.append(group.iter().map(|&(_, reader)| reader));
        }

        Self {
            program: Arc::new(program),
            flatten_map,
            node_lists,
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
        self.footprints
            .get(&node_id)
            .map_or(&[][..], |&range| &self.node_lists[range])
    }

    /// The nodes reading `node_idx`'s outputs. Empty when nothing does.
    fn consumers_of(&self, node_idx: NodeIdx) -> &[NodeIdx] {
        &self.node_lists[self.consumers[node_idx]]
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
    ///
    /// `None` where the node covers no compiled work — a boundary node, a
    /// definition no instance reaches, or a program not built yet. There is
    /// nothing to answer from, so the caller keeps whatever the authoring
    /// graph alone tells it.
    pub fn is_sink(&self, node_id: NodeId) -> Option<bool> {
        let footprint = self.footprint(node_id);
        (!footprint.is_empty()).then(|| footprint.iter().any(|&idx| self.program.e_nodes[idx].sink))
    }

    /// Whether an authored node holds work that recomputes every run.
    ///
    /// An impure node has no content digest, so nothing keys a cache on it.
    /// A graph instance inherits that from its interior: one impure node in
    /// there is enough for the instance to stop being reusable as a whole,
    /// even though its pure upstream still caches.
    ///
    /// `None` on a node with no footprint, as in [`Self::is_sink`].
    pub fn is_impure(&self, node_id: NodeId) -> Option<bool> {
        let footprint = self.footprint(node_id);
        (!footprint.is_empty()).then(|| {
            footprint
                .iter()
                .any(|&idx| self.program.e_nodes[idx].behavior == FuncBehavior::Impure)
        })
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
        // Membership is a search, and a silently wrong one on an unsorted
        // footprint — the ascending order is `pack_groups`' pair sort.
        debug_assert!(footprint.is_sorted());
        let inside = |node_idx: &NodeIdx| footprint.binary_search(node_idx).is_ok();
        // What the instance exposes, taken from the record flatten kept
        // rather than inferred. "Its value leaves the footprint" is not
        // observable in the finished program: the `GraphOutput` edge that
        // carried it is gone, so an exposed producer that an interior node
        // also reads looked purely internal and dropped out of the seeds —
        // while a dead interior terminal, with no readers at all, stayed
        // in. The request then ran the wrong cone entirely.
        let exposed = self
            .exposed
            .get(&node_id)
            .map_or(&[][..], |&range| &self.node_lists[range]);
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

    /// Self-consistency of the freshly compiled artifact against the `Library`
    /// it was compiled from. The source graph is gone after flattening, so this
    /// validates each `e_node` against its func and checks binding integrity.
    /// The debug wrapper runs at compile (where the library is in hand); the
    /// library-free install-side checks live in [`Self::validate_installed`].
    pub(crate) fn validate(&self, library: &Library) -> Result<(), CompiledGraphValidationError> {
        let program = &self.program;
        self.flatten_map
            .validate(program.e_node_ids.iter().copied())?;
        for (e_node_id, e_node) in program.e_node_ids.iter().zip(program.e_nodes.iter()) {
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
            let events = program.events.get(e_node.events.range()).ok_or(
                CompiledGraphValidationError::EventRange {
                    e_node_id: *e_node_id,
                },
            )?;

            // Unreachable while `apply_subscriptions` mints every subscriber from a
            // successful id lookup — kept as the backstop if that stops holding.
            for e_event in events {
                if let Some(&subscriber) = e_event
                    .subscribers
                    .iter()
                    .find(|s| s.idx() >= program.e_nodes.len())
                {
                    return Err(CompiledGraphValidationError::MissingEventSubscriber {
                        e_node_id: *e_node_id,
                        subscriber,
                    });
                }
            }

            for e_input in inputs {
                if let ExecutionBinding::Bind(e_addr) = &e_input.binding {
                    // Unreachable while `intern_bindings` mints every address from a
                    // successful id lookup — kept as the backstop if that stops holding.
                    let target_e_node = program.e_nodes.get(e_addr.node_idx).ok_or(
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
        if cache.slot_count() != self.program.e_nodes.len() {
            return Err(InstalledGraphValidationError::NodeSet);
        }

        for ((e_node_id, e_node), slot) in self
            .program
            .e_node_ids
            .iter()
            .zip(self.program.e_nodes.iter())
            .zip(cache.slots())
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

    /// Debug-only assert form of [`Self::validate_installed`].
    pub(crate) fn validate_installed_debug(&self, cache: &RuntimeCache) {
        if !is_debug() {
            return;
        }
        self.validate_installed(cache)
            .expect("installed compiled graph invariant violated");
    }
}

/// The compile entry point, owning reusable `Flattener` traversal scratch.
/// Hosts keep one per compile site (e.g. darkroom's `Engine`); the produced
/// [`CompiledGraph`] is always fresh and can be shared with the worker in an
/// [`Arc`].
#[derive(Debug, Default)]
pub struct Compiler {
    flattener: Flattener,
}

impl Compiler {
    /// Compile `graph` against `library`: validate, flatten composites into a
    /// flat func-only program, and resolve the output-type pool. Pure CPU on
    /// the caller's thread; the result is
    /// installed into an engine (`ExecutionEngine::install`)
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
        let mut program = Program::default();
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
    use crate::execution::flatten::map::internals::FlattenMapBuilder;
    use crate::execution::identity::ExecutionNodeId;
    use crate::execution::program::Program;
    use crate::graph::address::NodeId;

    impl CompiledGraph {
        /// Every execution node an authored node covers, in ascending id
        /// order — `CompiledGraph::footprint` spelled out.
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

    /// A [`CompiledGraph`] over an empty program, carrying only attribution
    /// — the published half of `FlattenMapBuilder`, which owns the
    /// leaf-building itself.
    #[derive(Debug)]
    pub struct CompiledGraphBuilder {
        flatten_map: FlattenMapBuilder,
    }

    impl CompiledGraphBuilder {
        pub fn new() -> Self {
            Self {
                flatten_map: FlattenMapBuilder::new(),
            }
        }

        pub fn insert_leaf(
            &mut self,
            e_node_id: ExecutionNodeId,
            instances: impl IntoIterator<Item = NodeId>,
            node_id: NodeId,
        ) {
            self.flatten_map.insert_leaf(e_node_id, instances, node_id);
        }

        pub fn build(self) -> Arc<CompiledGraph> {
            Arc::new(CompiledGraph::indexed(
                Program::default(),
                self.flatten_map.build(),
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
