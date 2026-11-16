//! Phase 1 of the pipeline, split off the engine so hosts compile on their own
//! thread: walk the authoring [`Graph`] straight into a self-contained
//! [`CompiledGraph`] the worker installs as-is.
//!
//! A compile has no expected failure. The host reconciles a document against
//! the library at load and admits each edit after, so the graph a compile gets
//! holds only nodes that resolve and wiring
//! [`Graph::validate_with`] accepts — which the compile asserts up front.
//!
//! The walk is a copy with two jobs it is the only place to do: resolving each
//! binding to the producer it names, and interning every id-named reference
//! into the dense index space.
//!
//! # Decisions
//!
//! - **One release assert checks the contract.** [`Compiler::compile`] is
//!   public API off the hot path, so it asserts [`Graph::validate_with`] in
//!   every build: a host's bug stops on the host's thread naming the bad node
//!   or binding, not in a lambda on the worker that misreads an input. The
//!   checks inside the walk and the planner's cycle check stay
//!   `debug_assert!`. Refused: a `CompileError`, as a graph the host keeps has
//!   no expected way to fail.
//!
//! **The sort comes first.** [`Graph::iter`] is a `HashMap` walk, so its order is not one an
//! artifact may depend on. Sorting the ids *before* emitting anything settles the dense index
//! space up front: a node's `NodeIdx` is its position in that sort, so a binding can name its
//! producer's index the moment the walk resolves it. Stable identity crosses into the dense index
//! space in one direction only — [`NodeId`]/[`OutputPort`] in, `NodeIdx`/`OutputAddr` out.
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

use crate::containers::col::{Col, Idx, Span};
use crate::exec::compile::compiled_graph::{
    CompiledGraph, ExecBinding, ExecEvent, ExecInput, ExecNode,
};
use crate::exec::idx::{NodeIdx, OutputAddr};
use crate::graph::Graph;
use crate::graph::binding::Binding;
use crate::graph::identity::{InputPort, NodeId, OutputPort};
use crate::graph::program_id::ProgramId;
use crate::library::Library;

/// One node awaiting placement: its id, which the sort keys on, and the one fact about it a
/// *subscription* may need before the walk reaches the node itself.
#[derive(Debug)]
struct PendingNode {
    node_id: NodeId,
    /// The node is disabled: it fires no events and receives none, which a
    /// subscription is checked against before its subscriber is emitted.
    disabled: bool,
}

/// How many ports the program will hold, totalled by the placement from the same declarations the
/// walk is about to read, so each variable-length pool is allocated once instead of doubling its
/// way to a size the placement already knew.
#[derive(Debug, Default, Clone, Copy)]
struct PortTotals {
    inputs: usize,
    outputs: usize,
    events: usize,
}

/// The compile entry point, owning the placement buffer the walk would otherwise allocate per
/// compile.
///
/// The buffer is kept between compiles for its capacity alone: it is refilled before it is read,
/// so a compile observes nothing of the last one. Hosts keep one per compile site, and what a
/// compile then allocates is exactly the artifact: five columns, each sized from the placement
/// and filled once.
///
/// The [`CompiledGraph`] cannot be reused, since the engine, its runtime cache, and the GUI hold
/// handles to the previous one while the next compile runs — so it is always fresh and can be
/// shared with the worker in an [`Arc`].
#[derive(Debug, Default)]
pub struct Compiler {
    /// Every node, sorted into the dense index space — *the* placement, read by the walk in both
    /// directions. A node's `NodeIdx` is its position here, and being sorted by id it answers the
    /// reverse too ([`idx`](Self::idx)). The artifact's id column is a projection of this.
    placed: Vec<PendingNode>,
}

impl Compiler {
    /// Compile `graph` against `library` into a func-only program. Pure CPU on
    /// the caller's thread; the result is installed into an engine
    /// (`ExecEngine::install`), typically across the worker channel.
    ///
    /// # Panics
    ///
    /// If `graph` breaks the load contract against `library` ([`Graph::validate_with`]). Checked
    /// in every build, so a host's bug stops here naming the bad node or binding.
    pub fn compile(&mut self, graph: &Graph, library: &Library) -> CompiledGraph {
        if let Err(breach) = graph.validate_with(library) {
            panic!("a host compiled a graph that breaks the load contract: {breach}");
        }
        let compiled = self.walk(graph, library);
        validate::validate_debug(&compiled, library);
        compiled
    }

    /// The walk itself, over a graph [`Self::compile`] has already validated.
    ///
    /// The artifact is stood up empty at its final size and written in place, so a field is named
    /// once, where its capacity is settled, and nothing outside this call can observe it
    /// part-built.
    ///
    /// `library` is a compile-time input like the graph: every port, lambda, and flag a node needs
    /// is copied out here (the lambdas are `Arc`s, a refcount bump), which makes the artifact
    /// self-contained.
    fn walk(&mut self, graph: &Graph, library: &Library) -> CompiledGraph {
        let totals = self.place_nodes(graph, library);

        // Sized from the placement, so each `append` below is a copy into space the artifact
        // already owns.
        let mut compiled = CompiledGraph {
            program_ids: Col::with_capacity(self.placed.len()),
            e_nodes: Col::with_capacity(self.placed.len()),
            inputs: Col::with_capacity(totals.inputs),
            output_count: 0,
            events: Col::with_capacity(totals.events),
            subscribers: Col::with_capacity(graph.subscriptions().len()),
            codecs: Arc::clone(library.codecs()),
        };
        // The id column is a projection of the placement; the walk resolves ids against `placed`.
        compiled.program_ids.append(
            self.placed
                .iter()
                .map(|placed| ProgramId::from(placed.node_id)),
        );

        for pos in 0..self.placed.len() {
            let node_id = self.placed[pos].node_id;
            let node = graph
                .find(node_id)
                .expect("the placement names this graph's nodes");

            // A func and a special node both resolve to a `&Func` spec, so the body is shared.
            let func = node.func(library);
            let special = node.special();

            // Every port is read fresh from the func each build, so the artifact is the library's
            // as given, in its order: the program addresses a port by position, and each binding
            // is found by the id the declaration gives the port. Each input is resolved as it is
            // appended, so it is whole on entering the pool.
            let node_inputs = compiled
                .inputs
                .append(func.inputs().iter().map(|input| ExecInput {
                    required: input.required,
                    fs_path: input.data_type.is_path(),
                    binding: self.lower(
                        graph,
                        library,
                        graph.bindings.get(&InputPort::new(node_id, input.id)),
                    ),
                }));

            let output_count =
                u32::try_from(func.outputs().len()).expect("a func's output count fits in u32");
            let node_outputs = Span::new(compiled.output_count, output_count);
            compiled.output_count += output_count;

            // Each event lands with its subscribers, resolved through the placement, which already
            // covers nodes the walk has not reached. The subscriptions sort by event id, which
            // need not be the declaration's order, so each event takes its own from the node's.
            let node_events = compiled.events.append(func.events().iter().map(|event| {
                let subscribers = compiled.subscribers.append(
                    graph
                        .subscriptions_of(node_id)
                        .filter(|sub| sub.event.event == event.id)
                        .filter(|sub| !node.disabled && !self.placement(sub.subscriber).disabled)
                        .map(|sub| self.idx(sub.subscriber)),
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
                disabled: node.disabled,
                behavior: func.behavior(),
                cache: node.cache,
                special,
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

    /// Settle the dense index space: the artifact's id column and the reverse map.
    ///
    /// Id order rather than `Graph::iter`'s, so the artifact is deterministic. Ids come from a map
    /// keyed by them, so the sort cannot produce a duplicate. The returned pool totals ride along
    /// because this pass already holds every declaration the walk will read.
    fn place_nodes(&mut self, graph: &Graph, library: &Library) -> PortTotals {
        let placed = &mut self.placed;
        placed.clear();
        placed.reserve_exact(graph.len());
        let mut totals = PortTotals::default();
        for node in graph.iter() {
            let func = node.func(library);
            totals.inputs += func.inputs().len();
            totals.outputs += func.outputs().len();
            totals.events += func.events().len();
            placed.push(PendingNode {
                node_id: node.node_id,
                disabled: node.disabled,
            });
        }
        assert!(
            u32::try_from(placed.len()).is_ok(),
            "program node count must fit in u32"
        );
        assert!(
            u32::try_from(totals.outputs).is_ok(),
            "program output count must fit in u32"
        );
        placed.sort_unstable_by_key(|placed| placed.node_id);
        totals
    }

    /// Where `node_id` landed in the dense index space.
    ///
    /// A binary search rather than a map beside it: the sort left `placed` ordered by id. Every id
    /// the walk resolves came out of the graph the placement covers, so a miss is a compile bug.
    fn idx(&self, node_id: NodeId) -> NodeIdx {
        let pos = self
            .placed
            .binary_search_by_key(&node_id, |placed| placed.node_id)
            .expect("the placement covers every node the graph holds");
        NodeIdx::from_idx(pos)
    }

    fn placement(&self, node_id: NodeId) -> &PendingNode {
        &self.placed[self.idx(node_id).idx()]
    }

    /// What an input lowers to: its constant, the producer it is wired to, or
    /// nothing.
    fn lower(&self, graph: &Graph, library: &Library, binding: Option<&Binding>) -> ExecBinding {
        match binding {
            None => ExecBinding::None,
            Some(Binding::Const(value)) => ExecBinding::Const(value.clone()),
            Some(Binding::Wire(source)) => self.resolve(graph, library, *source),
        }
    }

    /// Intern an output reference into the dense [`OutputAddr`] the producer landed at — the one
    /// place a producer id is hashed and its port's id mapped to its position, once per compile
    /// instead of once per run.
    fn resolve(&self, graph: &Graph, library: &Library, port: OutputPort) -> ExecBinding {
        let port_idx = graph
            .find(port.node_id)
            .expect("an admitted wire's producer is in the graph")
            .func(library)
            .ports()
            .output_idx(port.port)
            .expect("an admitted wire reads an output its producer declares");
        ExecBinding::Wire(OutputAddr {
            node_idx: self.idx(port.node_id),
            port_idx: u32::try_from(port_idx).expect("a func's outputs fit in u32"),
        })
    }
}

#[cfg(test)]
mod tests;
