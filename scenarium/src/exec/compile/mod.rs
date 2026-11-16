//! Phase 1 of the pipeline, split off the engine so hosts compile on their own
//! thread: walk the authoring document — its [`GraphSet`] — straight into a self-contained
//! [`CompiledGraph`] the worker installs as-is.
//!
//! A compile has no expected failure. The host reconciles a document against
//! the library at load and admits each edit after, so the set a compile
//! gets holds only nodes that resolve and wiring
//! [`GraphSet::validate_with`] accepts — which the compile asserts up front.
//!
//! The walk is a copy with three jobs it is the only place to do: expanding each
//! instance into its subgraph's nodes, resolving each binding to the producer it
//! names across every boundary between, and interning every id-named reference
//! into the dense index space.
//!
//! # Decisions
//!
//! - **One release assert checks the contract.** [`Compiler::compile`] is
//!   public API off the hot path, so it asserts [`GraphSet::validate_with`] in
//!   every build: a host's bug stops on the host's thread naming the bad node
//!   or binding, not in a lambda on the worker that misreads an input. The
//!   checks inside the walk and the planner's cycle check stay
//!   `debug_assert!`. Refused: a `CompileError`, as a graph the host keeps has
//!   no expected way to fail.
//!
//! - **The compile flattens; plan and execute see no hierarchy.** An instance and a boundary
//!   node emit nothing, and each node inside an instance is a program node of its own, named by
//!   its [`ProgramId`]: so each caches, reports and is seeded as any node is. A binding that
//!   reaches an instance or a boundary node is replaced by what the other side holds until it
//!   names a func's output, a constant or nothing, so a passthrough costs nothing and output
//!   demand prunes across the boundary. As Unreal expands each macro instance "into a unique set
//!   of nodes during compilation".
//!
//! - **A disabled instance disables every node inside it.** An instance has no cache mode: the
//!   nodes inside carry their own.
//!
//! **The sort comes first.** [`Graph::iter`](crate::Graph::iter) is a `HashMap` walk, so its order is not one an
//! artifact may depend on. Sorting the ids *before* emitting anything settles the dense index
//! space up front: a node's `NodeIdx` is its position in that sort, so a binding can name its
//! producer's index the moment the walk resolves it. Stable identity crosses into the dense index
//! space in one direction only — [`NodeId`]/[`OutputPort`](crate::graph::identity::OutputPort)
//! in, `NodeIdx`/`OutputAddr` out.
//!
//! The program is immutable for the life of the install, so each step below produces its part
//! complete rather than reserving space for a later pass.
//!
//! This is phase 1 of the three [`exec`](crate::exec) describes.

pub(crate) mod compiled_graph;
pub(crate) mod consumer_cone;
pub(crate) mod error;
mod validate;

use std::sync::Arc;

use common::ErrorChain;
use hashbrown::HashMap;

use crate::containers::col::{Col, Idx, Span};
use crate::exec::compile::compiled_graph::{
    CompiledGraph, ExecBinding, ExecEvent, ExecInput, ExecNode,
};
use crate::exec::idx::{NodeIdx, OutputAddr};
use crate::graph::binding::Binding;
use crate::graph::graph_set::GraphSet;
use crate::graph::identity::{GraphId, InputPort, NodeId, SubgraphId};
use crate::graph::instance_path::InstancePath;
use crate::graph::node::{Boundary, NodeKind};
use crate::graph::node_decl::NodeDecl;
use crate::graph::program_id::ProgramId;
use crate::library::Library;

/// One program node awaiting placement: its id, which the sort keys on, the graph it was
/// authored in, and the one fact about it a *subscription* may need before the walk reaches the
/// node itself.
#[derive(Debug, Clone, Copy)]
struct PendingNode {
    program_id: ProgramId,
    graph: GraphId,
    /// The node is disabled, or lies inside a disabled instance: it fires no events and receives
    /// none, which a subscription is checked against before its subscriber is emitted.
    disabled: bool,
}

/// One graph the placement has yet to expand: the main graph, or a subgraph's under the
/// instance that places it.
#[derive(Debug, Clone, Copy)]
struct Frame {
    level: Level,
    /// The instance is disabled, or lies inside a disabled one.
    disabled: bool,
}

/// Where in the hierarchy a walk stands: the graph, and the instances down to it.
#[derive(Debug, Clone, Copy)]
struct Level {
    graph: GraphId,
    path: InstancePath,
}

impl Level {
    const MAIN: Self = Self {
        graph: GraphId::Main,
        path: InstancePath::MAIN,
    };

    /// The program id of `node_id` of this level's graph.
    fn program_id(self, node_id: NodeId) -> ProgramId {
        self.path.node(node_id)
    }

    /// The level inside instance `node_id` of this level's graph, which places subgraph `graph`.
    fn enter(self, node_id: NodeId, graph: GraphId) -> Self {
        Self {
            graph,
            path: self.path.child(node_id),
        }
    }
}

/// How many ports the program will hold, totalled by the placement from the same declarations the
/// walk is about to read, so each variable-length pool is allocated once instead of doubling its
/// way to a size the placement already knew.
#[derive(Debug, Default, Clone, Copy)]
struct PortTotals {
    inputs: usize,
    outputs: usize,
    events: usize,
    subscribers: usize,
}

/// The compile entry point, owning the placement buffers the walk would otherwise allocate per
/// compile.
///
/// The buffers are kept between compiles for their capacity alone: each is refilled before it is
/// read, so a compile observes nothing of the last one. Hosts keep one per compile site, and what
/// a compile then allocates is exactly the artifact: five columns, each sized from the placement
/// and filled once.
///
/// The [`CompiledGraph`] cannot be reused, since the engine, its runtime cache, and the GUI hold
/// handles to the previous one while the next compile runs — so it is always fresh and can be
/// shared with the worker in an [`Arc`].
#[derive(Debug, Default)]
pub struct Compiler {
    /// Every program node, sorted into the dense index space — *the* placement, read by the walk
    /// in both directions. A node's `NodeIdx` is its position here, and being sorted by id it
    /// answers the reverse too ([`idx`](Self::idx)). The artifact's id column is a projection of
    /// this.
    placed: Vec<PendingNode>,
    /// The graphs the placement has yet to expand.
    frames: Vec<Frame>,
    /// The `Outputs` node of each subgraph the placement expanded, which a wire from an instance
    /// crosses into.
    outputs_nodes: HashMap<SubgraphId, NodeId>,
}

/// What a compile reads: the set and the library its nodes resolve against.
#[derive(Debug, Clone, Copy)]
struct Source<'a> {
    graphs: &'a GraphSet,
    library: &'a Library,
}

impl Compiler {
    /// Compile `graphs` against `library` into a func-only program. Pure CPU on the caller's
    /// thread; the result is installed into an engine (`ExecEngine::install`), typically across
    /// the worker channel.
    ///
    /// # Panics
    ///
    /// If the set breaks the load contract against `library` ([`GraphSet::validate_with`]).
    /// Checked in every build, so a host's bug stops here naming the bad node or binding.
    pub fn compile(&mut self, graphs: &GraphSet, library: &Library) -> CompiledGraph {
        if let Err(breach) = graphs.validate_with(library) {
            panic!(
                "a host compiled a document that breaks the load contract: {}",
                ErrorChain::new(&breach)
            );
        }
        let compiled = self.walk(Source { graphs, library });
        validate::validate_debug(&compiled, library);
        compiled
    }

    /// The walk itself, over a document [`Self::compile`] has already validated.
    ///
    /// The artifact is stood up empty at its final size and written in place, so a field is named
    /// once, where its capacity is settled, and nothing outside this call can observe it
    /// part-built.
    ///
    /// `library` is a compile-time input like the graphs: every port, lambda, and flag a node
    /// needs is copied out here (the lambdas are `Arc`s, a refcount bump), which makes the
    /// artifact self-contained.
    fn walk(&mut self, source: Source<'_>) -> CompiledGraph {
        let totals = self.place_nodes(source);

        // Sized from the placement, so each `append` below is a copy into space the artifact
        // already owns.
        let mut compiled = CompiledGraph {
            program_ids: Col::with_capacity(self.placed.len()),
            e_nodes: Col::with_capacity(self.placed.len()),
            inputs: Col::with_capacity(totals.inputs),
            output_count: 0,
            events: Col::with_capacity(totals.events),
            subscribers: Col::with_capacity(totals.subscribers),
            codecs: Arc::clone(source.library.codecs()),
        };
        // The id column is a projection of the placement; the walk resolves ids against `placed`.
        compiled
            .program_ids
            .append(self.placed.iter().map(|placed| placed.program_id));

        for pos in 0..self.placed.len() {
            let pending = self.placed[pos];
            let level = Level {
                graph: pending.graph,
                path: pending.program_id.path(),
            };
            let node_id = pending.program_id.node_id();
            let graph = source.graphs.graph(level.graph);
            let node = graph
                .find(node_id)
                .expect("the placement names this graph's nodes");

            // The placement keeps the nodes with a body alone, a func and a special node, which
            // both resolve to a `&Func` spec.
            let (NodeKind::Body { cache, .. }, NodeDecl::Func(func)) = (
                node.kind,
                node.decl(source.graphs.scope(source.library, level.graph)),
            ) else {
                unreachable!("the placement keeps only the nodes with a body");
            };

            // Every port is read fresh from the func each build, so the artifact is the library's
            // as given, in its order: the program addresses a port by position, and each binding
            // is found by the id the declaration gives the port. Each input is resolved as it is
            // appended, so it is whole on entering the pool.
            let node_inputs = compiled
                .inputs
                .append(func.inputs().iter().map(|func_input| ExecInput {
                    required: func_input.required,
                    fs_path: func_input.data_type.is_path(),
                    binding: self.lower(
                        source,
                        level,
                        graph.bindings.get(&InputPort::new(node_id, func_input.id)),
                    ),
                }));

            let output_count =
                u32::try_from(func.outputs().len()).expect("a func's output count fits in u32");
            let node_outputs = Span::new(compiled.output_count, output_count);
            compiled.output_count += output_count;

            // Each event lands with its subscribers, resolved through the placement, which already
            // covers nodes the walk has not reached. A subscription stays inside its graph, so
            // the subscriber shares the emitter's level. The subscriptions sort by event id, which
            // need not be the declaration's order, so each event takes its own from the node's.
            let node_events = compiled.events.append(func.events().iter().map(|event| {
                let subscribers = compiled.subscribers.append(
                    graph
                        .subscriptions_of(node_id)
                        .filter(|sub| sub.event.event == event.id)
                        .map(|sub| self.idx(level.program_id(sub.subscriber)))
                        .filter(|subscriber| {
                            !pending.disabled && !self.placed[subscriber.idx()].disabled
                        }),
                );
                ExecEvent {
                    subscribers,
                    lambda: event.event_lambda.clone(),
                }
            }));
            debug_assert!(
                graph
                    .subscriptions_of(node_id)
                    .all(|sub| func.event(sub.event.event).is_some()),
                "every subscription names an event its emitter declares"
            );

            compiled.e_nodes.push(ExecNode {
                sink: func.sink(),
                disabled: pending.disabled,
                behavior: func.behavior(),
                cache,
                special: node.special(),
                inputs: node_inputs,
                outputs: node_outputs,
                events: node_events,
                func_ref: func.func_ref(),
                lambda: func.lambda().clone(),
            });
        }

        // The placement counted these from the same declarations the walk read, so a mismatch is
        // the two disagreeing about the library.
        debug_assert_eq!(compiled.inputs.len(), totals.inputs);
        debug_assert_eq!(compiled.output_count as usize, totals.outputs);
        debug_assert_eq!(compiled.events.len(), totals.events);

        compiled
    }

    /// Settle the dense index space: expand every instance into the nodes of its subgraph, keep
    /// each node with a body, and sort them by program id.
    ///
    /// Id order rather than `Graph::iter`'s, so the artifact is deterministic. A program id is
    /// unique by construction, so the sort cannot produce a duplicate. The returned pool totals
    /// ride along because this pass already holds every declaration the walk will read.
    fn place_nodes(&mut self, source: Source<'_>) -> PortTotals {
        let Self {
            placed,
            frames,
            outputs_nodes,
        } = self;
        placed.clear();
        frames.clear();
        outputs_nodes.clear();
        frames.push(Frame {
            level: Level::MAIN,
            disabled: false,
        });
        let mut totals = PortTotals::default();
        while let Some(frame) = frames.pop() {
            let graph = source.graphs.graph(frame.level.graph);
            let scope = source.graphs.scope(source.library, frame.level.graph);
            for node in graph.iter() {
                let disabled = frame.disabled || node.disabled;
                match node.kind {
                    NodeKind::Body { .. } => {
                        let decl = node.decl(scope);
                        totals.inputs += decl.inputs().len();
                        totals.outputs += decl.outputs().len();
                        totals.events += decl.events().len();
                        totals.subscribers += graph.subscriptions_of(node.node_id).count();
                        placed.push(PendingNode {
                            program_id: frame.level.program_id(node.node_id),
                            graph: frame.level.graph,
                            disabled,
                        });
                    }
                    NodeKind::Instance(subgraph_id) => frames.push(Frame {
                        level: frame
                            .level
                            .enter(node.node_id, GraphId::Subgraph(subgraph_id)),
                        disabled,
                    }),
                    NodeKind::Boundary(Boundary::Outputs) => {
                        if let GraphId::Subgraph(id) = frame.level.graph {
                            outputs_nodes.insert(id, node.node_id);
                        }
                    }
                    NodeKind::Boundary(Boundary::Inputs) => {}
                }
            }
        }
        assert!(
            u32::try_from(placed.len()).is_ok(),
            "program node count must fit in u32"
        );
        assert!(
            u32::try_from(totals.outputs).is_ok(),
            "program output count must fit in u32"
        );
        placed.sort_unstable_by_key(|placed| placed.program_id);
        totals
    }

    /// Where `program_id` landed in the dense index space.
    ///
    /// A binary search rather than a map beside it: the sort left `placed` ordered by id. Every id
    /// the walk resolves names a node with a body the placement expanded, so a miss is a compile
    /// bug.
    fn idx(&self, program_id: ProgramId) -> NodeIdx {
        let pos = self
            .placed
            .binary_search_by_key(&program_id, |placed| placed.program_id)
            .expect("the placement covers every program node");
        NodeIdx::from_idx(pos)
    }

    /// What an input of a node at `level` lowers to: its constant, the program node output it is
    /// wired to, or nothing.
    ///
    /// A wire from an instance or an `Inputs` node names no program node, so it is followed
    /// across the boundary to what the other side holds under the same port id: into the
    /// subgraph, to the binding on its `Outputs` node, or out of it, to the binding on the
    /// instance. A wire to a func's output lands on that output's position. Each step follows a
    /// wire back toward its producer, and the document holds no cycle, so the walk ends at a
    /// func's output, a constant or nothing. A passthrough costs nothing, and output demand
    /// prunes across the boundary as anywhere else. As Unreal expands each macro instance "into a
    /// unique set of nodes during compilation".
    fn lower(
        &self,
        source: Source<'_>,
        mut level: Level,
        binding: Option<&Binding>,
    ) -> ExecBinding {
        let mut binding = binding;
        loop {
            let source_port = match binding {
                None => return ExecBinding::None,
                Some(Binding::Const(value)) => return ExecBinding::Const(value.clone()),
                Some(Binding::Wire(source_port)) => *source_port,
            };
            let producer = source
                .graphs
                .graph(level.graph)
                .find(source_port.node_id)
                .expect("an admitted wire's producer is in the graph");
            binding = match producer.kind {
                NodeKind::Body { .. } => {
                    let port_idx = producer
                        .decl(source.graphs.scope(source.library, level.graph))
                        .ports()
                        .output_idx(source_port.port)
                        .expect("an admitted wire reads an output its producer declares");
                    return ExecBinding::Wire(OutputAddr {
                        node_idx: self.idx(level.program_id(source_port.node_id)),
                        port_idx: u32::try_from(port_idx).expect("a func's outputs fit in u32"),
                    });
                }
                NodeKind::Instance(subgraph_id) => {
                    level = level.enter(source_port.node_id, GraphId::Subgraph(subgraph_id));
                    let outputs = self.outputs_nodes[&subgraph_id];
                    source
                        .graphs
                        .graph(level.graph)
                        .bindings
                        .get(&InputPort::new(outputs, source_port.port))
                }
                NodeKind::Boundary(Boundary::Inputs) => {
                    let instance = level
                        .path
                        .instance()
                        .expect("an `Inputs` node lies inside an instance");
                    let path = level.path.parent().expect("an instance lies in a graph");
                    let graph = source
                        .graphs
                        .graph_at(path)
                        .expect("an instance path names instances");
                    level = Level { graph, path };
                    source
                        .graphs
                        .graph(graph)
                        .bindings
                        .get(&InputPort::new(instance, source_port.port))
                }
                NodeKind::Boundary(Boundary::Outputs) => {
                    unreachable!("the `Outputs` node declares no outputs")
                }
            };
        }
    }
}

#[cfg(test)]
mod tests;
