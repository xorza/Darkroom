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
//! **The sort comes first.** [`Graph::iter`] is a `HashMap` walk, so the order
//! nodes are reached in is not an order an artifact may depend on. Sorting the
//! ids *before* emitting anything settles the whole dense index space up front:
//! a node's `NodeIdx` is its position in that sort, so a binding can name its
//! producer's index the moment the walk resolves it, and no later pass has to
//! revisit a column to translate one. This is where stable identity meets the
//! dense index space, and it crosses in one direction only — stable
//! [`NodeId`]/[`OutputPort`] in, dense `NodeIdx`/`OutputAddr` out.
//!
//! Everything the walk produces is final. The program is immutable for the life
//! of the install — no later pass fills a field in — so each step below produces
//! its part complete rather than reserving space for it.
//!
//! This is phase 1 of the three [`exec`](crate::exec) describes.

pub(crate) mod compiled_graph;
pub(crate) mod consumer_cone;
pub(crate) mod error;
mod validate;

use std::iter;
use std::sync::Arc;

use crate::containers::col::{Col, Idx, Span};
use crate::data::data_type::DataType;
use crate::exec::compile::compiled_graph::{
    CompiledGraph, ExecBinding, ExecEvent, ExecInput, ExecNode,
};
use crate::exec::idx::{NodeIdx, OutputAddr};
use crate::graph::Graph;
use crate::graph::binding::Binding;
use crate::graph::identity::{EventPort, InputPort, NodeId, OutputPort};
use crate::library::Library;

/// One node awaiting placement: its id, which the sort keys on, and the one
/// fact about it a *subscription* may need before the walk reaches the node
/// itself.
///
/// Not to be confused with the fixture builder's `Placed`, which is where a
/// node *landed* — this is what the walk knows before it gets there.
#[derive(Debug)]
struct PendingNode {
    node_id: NodeId,
    /// The node is disabled: it fires no events and receives none, which a
    /// subscription is checked against before its subscriber is emitted.
    disabled: bool,
}

/// How many ports the program will hold, totalled by the placement from the
/// same declarations the walk is about to read.
///
/// Every pool the walk fills is variable-length, so without these each one
/// doubles its way to a size the placement already knew: a thousand-node graph
/// pays a dozen reallocations and copies per column for a number that was one
/// addition away.
#[derive(Debug, Default, Clone, Copy)]
struct PortTotals {
    inputs: usize,
    outputs: usize,
    events: usize,
}

/// The compile entry point, owning every buffer the walk would otherwise
/// allocate per compile.
///
/// The fields below are kept between compiles for their capacity alone: each is
/// refilled where it is first written, so a compile can observe nothing of the
/// last one. That is the whole reason this is a value a caller holds rather than
/// a function it calls — hosts keep one per compile site (e.g. darkroom's
/// `Engine`), and what a compile then allocates is exactly the artifact: five
/// columns, each sized from the placement and filled once.
///
/// The [`CompiledGraph`] is the one thing here that cannot be reused, since the
/// engine, its runtime cache, and the GUI all hold handles to the previous one
/// while the next compile runs — so it is always fresh and can be shared with
/// the worker in an [`Arc`].
#[derive(Debug, Default)]
pub struct Compiler {
    /// Every node, sorted into the dense index space — *the* placement, which
    /// the walk reads for both directions. A node's `NodeIdx` is its position
    /// here, and being sorted by id it answers the reverse too
    /// ([`idx`](Self::idx)). The artifact's own id column is a projection of
    /// this, not a second copy anything looks things up in.
    placed: Vec<PendingNode>,
}

impl Compiler {
    /// Compile `graph` against `library` into a func-only program. Pure CPU on
    /// the caller's thread; the result is installed into an engine
    /// (`ExecEngine::install`), typically across the worker channel.
    ///
    /// # Panics
    ///
    /// If `graph` breaks the load contract against `library`
    /// ([`Graph::validate_with`]). Checked in every build, so a host's bug
    /// stops here naming the bad node or binding, rather than in a lambda on
    /// the worker that misreads an input.
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
    /// The artifact is stood up empty at its final size and written in place —
    /// no column is staged beside it and moved in afterwards, so a field is
    /// named once, where its capacity is settled. It is a local until it is
    /// returned, so nothing outside this call can observe it part-built.
    ///
    /// `library` is a compile-time input like the graph beside it: every port,
    /// lambda, and flag a node needs is copied out here (the lambdas are `Arc`s,
    /// so a copy is a refcount bump), which is what leaves the artifact
    /// self-contained.
    fn walk(&mut self, graph: &Graph, library: &Library) -> CompiledGraph {
        let totals = self.place_nodes(graph, library);

        // Sized from the placement, so each column allocates once and every
        // `append` below is a copy into space the artifact already owns.
        let mut compiled = CompiledGraph {
            node_ids: Col::with_capacity(self.placed.len()),
            e_nodes: Col::with_capacity(self.placed.len()),
            inputs: Col::with_capacity(totals.inputs),
            output_count: 0,
            events: Col::with_capacity(totals.events),
            subscribers: Col::with_capacity(graph.subscriptions().len()),
            codecs: Arc::clone(library.codecs()),
        };
        // Ordered by emitter, then event, like the events the walk appends, so
        // each event's subscribers are the run at the front of what is left.
        let mut subscriptions = graph.subscriptions().peekable();
        // The id column is a projection of the placement, written once and never
        // read back — the walk resolves ids against `placed` itself.
        compiled
            .node_ids
            .append(self.placed.iter().map(|placed| placed.node_id));

        for pos in 0..self.placed.len() {
            let node_id = self.placed[pos].node_id;
            let node = graph
                .find(node_id)
                .expect("the placement names this graph's nodes");

            // A func and a special node both resolve to a `&Func` spec and emit
            // one node, so the body below is shared.
            let func = node.func(library);
            let special = node.special();

            // Every port is read fresh from the func each build, never carried
            // over from the last one, so the artifact is the library's as
            // given.
            //
            // Each input is resolved as it is appended, so it is whole the moment
            // it enters the pool rather than being revisited by index afterwards.
            let node_inputs = compiled.inputs.append(func.inputs().iter().enumerate().map(
                |(port_idx, func_input)| ExecInput {
                    required: func_input.required,
                    stamps_fs_path: matches!(&func_input.data_type, DataType::FsPath(_)),
                    binding: self.lower(graph.bindings.get(&InputPort::new(node_id, port_idx))),
                },
            ));

            let output_count =
                u32::try_from(func.outputs().len()).expect("a func's output count fits in u32");
            let node_outputs = Span::new(compiled.output_count, output_count);
            compiled.output_count += output_count;

            // Each event lands with its subscribers, resolved through the
            // placement, which already covers nodes the walk has not reached.
            let node_events = compiled.events.append(func.events().iter().enumerate().map(
                |(event_idx, event)| {
                    let port = EventPort::new(node_id, event_idx);
                    #[expect(clippy::debug_assert_with_mut_call, reason = "a peek moves nothing")]
                    {
                        debug_assert!(
                            subscriptions.peek().is_none_or(|sub| sub.event >= port),
                            "every subscription names an event its emitter declares"
                        );
                    }
                    let subscribers = compiled.subscribers.append(
                        iter::from_fn(|| subscriptions.next_if(|sub| sub.event == port))
                            .filter(|sub| {
                                !node.disabled && !self.placement(sub.subscriber).disabled
                            })
                            .map(|sub| self.idx(sub.subscriber)),
                    );
                    ExecEvent {
                        subscribers,
                        lambda: event.event_lambda.clone(),
                    }
                },
            ));

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

        #[expect(clippy::debug_assert_with_mut_call, reason = "a peek moves nothing")]
        {
            debug_assert!(
                subscriptions.peek().is_none(),
                "every subscription names an event its emitter declares"
            );
        }
        // The placement counted these from the same declarations the walk read,
        // so a mismatch is the two disagreeing about the library — and every
        // column above would have silently regrown to cover it.
        debug_assert_eq!(compiled.inputs.len(), totals.inputs);
        debug_assert_eq!(compiled.output_count as usize, totals.outputs);
        debug_assert_eq!(compiled.events.len(), totals.events);

        compiled
    }

    /// Settle the dense index space: the artifact's id column and the reverse
    /// map.
    ///
    /// Id order rather than `Graph::iter`'s, so the artifact is deterministic
    /// however the walk happens to reach the nodes. Ids come from a map keyed by
    /// them, so the sort cannot produce a duplicate.
    ///
    /// The returned pool totals ride along for free: this pass already holds
    /// every declaration the walk is about to read, so counting the ports here
    /// is what lets each column be allocated once at its final size.
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
                node_id: node.id,
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
    /// A binary search of the placement rather than a map beside it: the sort
    /// that assigned the indices left `placed` ordered by id, so the ordering
    /// already *is* the lookup. Every id the walk resolves came out of the graph
    /// the placement covers, so a miss is a compile bug.
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
    fn lower(&self, binding: Option<&Binding>) -> ExecBinding {
        match binding {
            None => ExecBinding::None,
            Some(Binding::Const(value)) => ExecBinding::Const(value.clone()),
            Some(Binding::Wire(source)) => self.resolve(*source),
        }
    }

    /// Intern an output reference into the dense [`OutputAddr`] the producer
    /// landed at — the one place a producer id is ever hashed, once per compile
    /// instead of once per run.
    fn resolve(&self, port: OutputPort) -> ExecBinding {
        ExecBinding::Wire(OutputAddr {
            node_idx: self.idx(port.node_id),
            port_idx: port.port_idx as u32,
        })
    }
}

#[cfg(test)]
mod tests;
