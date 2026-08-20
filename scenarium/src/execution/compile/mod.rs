//! Phase 1 of the pipeline, split off the engine so hosts compile on their own
//! thread: validate the authoring [`Graph`] against the [`Library`], then walk
//! it straight into a self-contained [`CompiledGraph`] the worker installs
//! as-is. Compile errors surface synchronously at the call site — a graph that
//! doesn't compile is never sent, so the worker's install is infallible and a
//! running event loop is never disturbed by a bad edit.
//!
//! The walk is a copy with four jobs it is the only place to do: resolving each
//! binding to the producer it names, gating it against the declared types,
//! stamping each output with the effective type the same pass resolved, and
//! interning every id-named reference into the dense index space.
//! The *type gate* is drift tolerance — a wire whose resolved source type no
//! longer fits the consumer, or a const that no longer satisfies it, lowers as
//! unbound rather than severing authored wiring.
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
//! This is phase 1 of the three [`execution`](crate::execution) describes.

pub(crate) mod compiled_graph;
pub(crate) mod consumer_cone;
pub(crate) mod error;
mod validate;

use crate::DataType;
use crate::containers::column::Column;
use crate::execution::compile::compiled_graph::{
    CompiledGraph, ExecutionBinding, ExecutionEvent, ExecutionInput, ExecutionNode,
};
use crate::execution::compile::error::CompileError;
use crate::execution::identity::{NodeIdx, OutputAddr};
use crate::graph::func::{Func, FuncInput};
use crate::graph::identity::{InputPort, NodeId, OutputPort};
use crate::graph::node::NodeKind;
use crate::graph::node::special::SpecialNode;
use crate::graph::output_types::OutputTypes;
use crate::graph::{Binding, Graph};
use crate::library::Library;

/// One node awaiting placement: its id, which the sort keys on, and the one
/// fact about it a *consumer* may need before the walk reaches the node itself.
///
/// Not to be confused with the fixture builder's `Placed`, which is where a
/// node *landed* — this is what the walk knows before it gets there.
#[derive(Debug)]
struct PendingNode {
    node_id: NodeId,
    /// The node's declared output count. A binding is range-checked against it,
    /// and the producer's own `outputs` run does not exist until the walk emits
    /// that node — which the id order may put after the consumer naming it.
    outputs: u32,
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
/// the worker in an [`Arc`](std::sync::Arc).
#[derive(Debug, Default)]
pub struct Compiler {
    /// Every node, sorted into the dense index space — *the* placement, which
    /// the walk reads for both directions. A node's `NodeIdx` is its position
    /// here, and being sorted by id it answers the reverse too
    /// ([`idx`](Self::idx)). The artifact's own id column is a projection of
    /// this, not a second copy anything looks things up in.
    placed: Vec<PendingNode>,
    /// Every output type of the graph, filled before the walk. Held here
    /// because the type gate runs once per bound input, and resolving one port
    /// at a time cost a walk per edge.
    output_types: OutputTypes,
}

impl Compiler {
    /// Compile `graph` against `library`: validate, then walk into a func-only
    /// program with its output-type pool resolved. Pure CPU on the caller's
    /// thread; the result is installed into an engine
    /// (`ExecutionEngine::install`), typically across the worker channel.
    ///
    /// A rejection is returned, not logged. It is an ordinary outcome of editing
    /// against an evolving library, and the caller is the one that knows whether
    /// this compile was a user action to surface or a speculative one to
    /// swallow — darkroom, for instance, puts the same message on its status
    /// bar.
    pub fn compile(
        &mut self,
        graph: &Graph,
        library: &Library,
    ) -> Result<CompiledGraph, CompileError> {
        // Validate before building anything: the graph+library pair is untrusted
        // input, and a passing check lets the walk below resolve every reference
        // infallibly.
        if let Err(e) = graph.validate_with(library) {
            return Err(CompileError {
                message: e.to_string(),
            });
        }

        let compiled = self.walk(graph, library);
        validate::validate_debug(&compiled, library);
        Ok(compiled)
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
        self.output_types.update(graph, library);
        let totals = self.place_nodes(graph, library);

        // Sized from the placement, so each column allocates once and every
        // `append` below is a copy into space the artifact already owns.
        let mut compiled = CompiledGraph {
            node_ids: Column::with_capacity(self.placed.len()),
            e_nodes: Column::with_capacity(self.placed.len()),
            inputs: Column::with_capacity(totals.inputs),
            outputs: Column::with_capacity(totals.outputs),
            events: Column::with_capacity(totals.events),
        };
        // The id column is a projection of the placement, written once and never
        // read back — the walk resolves ids against `placed` itself.
        compiled
            .node_ids
            .append(self.placed.iter().map(|placed| placed.node_id));

        for position in 0..self.placed.len() {
            let node_id = self.placed[position].node_id;
            let node = graph
                .find(node_id)
                .expect("the placement names this graph's nodes");

            // A func and a special node both resolve to a `&Func` spec and emit
            // one node — the spec is the only difference (`library` vs. the
            // hardcoded `SpecialNode::func`), so the body below is shared.
            let (func, special): (&Func, Option<SpecialNode>) = match &node.kind {
                NodeKind::Func(func_id) => (
                    library
                        .by_id(*func_id)
                        .expect("func resolved by validate_with"),
                    None,
                ),
                NodeKind::Special(special) => (special.func(), Some(*special)),
            };

            // Every port is read fresh from the func each build (never carried
            // over from the last one): the library can evolve between updates —
            // a changed `required` flag, a grown input list, a retyped output —
            // and this is where that lands.
            //
            // Each input is resolved as it is appended, so it is whole the moment
            // it enters the pool rather than being revisited by index afterwards.
            let node_inputs = compiled.inputs.append(func.inputs.iter().enumerate().map(
                |(port_idx, func_input)| ExecutionInput {
                    required: func_input.required,
                    stamps_fs_path: matches!(&func_input.data_type, DataType::FsPath(_)),
                    binding: self.typed_binding(
                        library,
                        func_input,
                        graph.bindings.get(&InputPort::new(node_id, port_idx)),
                    ),
                },
            ));

            // The effective type of each output, straight off the table filled
            // above — the same answer the editor paints, so the program and the
            // canvas agree about a wildcard by construction rather than by two
            // walks happening to match. A port the table missed is library
            // drift, and `Any` is the right answer for one.
            let node_outputs = compiled
                .outputs
                .append((0..func.outputs.len()).map(|port_idx| {
                    self.output_types
                        .get(OutputPort::new(node_id, port_idx))
                        .cloned()
                        .unwrap_or_default()
                }));

            // Each event port lands with the half its own declaration answers
            // for. The other half — who subscribes — belongs to the *emitter*,
            // which the id order may put after the subscriber, so
            // `wire_subscriptions` fills those lists once every node is placed.
            let node_events =
                compiled
                    .events
                    .append(func.events.iter().map(|event| ExecutionEvent {
                        subscribers: Vec::new(),
                        lambda: event.event_lambda.clone(),
                    }));

            compiled.e_nodes.push(ExecutionNode {
                sink: func.sink,
                disabled: node.disabled,
                behavior: func.behavior,
                cache: node.cache,
                special,
                inputs: node_inputs,
                outputs: node_outputs,
                events: node_events,
                func_id: func.id,
                lambda: func.lambda.clone(),
            });
        }

        self.wire_subscriptions(graph, &mut compiled);

        // The placement counted these from the same declarations the walk read,
        // so a mismatch is the two disagreeing about the library — and every
        // column above would have silently regrown to cover it.
        debug_assert_eq!(compiled.inputs.len(), totals.inputs);
        debug_assert_eq!(compiled.outputs.len(), totals.outputs);
        debug_assert_eq!(compiled.events.len(), totals.events);

        compiled
    }

    /// Settle the dense index space: the artifact's id column and the reverse
    /// map, plus the output count the walk range-checks bindings against.
    ///
    /// Id order rather than `Graph::iter`'s, so the artifact is deterministic
    /// however the walk happens to reach the nodes. Ids come from a map keyed by
    /// them, so the sort cannot produce a duplicate.
    ///
    /// The per-node output count is taken here rather than during the walk
    /// because a wire is range-checked against its *producer*, which the id
    /// order may put after the consumer that names it.
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
            let func = graph
                .node_func(&node, library)
                .expect("func resolved by validate_with");
            totals.inputs += func.inputs.len();
            totals.outputs += func.outputs.len();
            totals.events += func.events.len();
            placed.push(PendingNode {
                node_id: node.id,
                outputs: func.outputs.len() as u32,
            });
        }
        assert!(
            u32::try_from(placed.len()).is_ok(),
            "program node count must fit in u32"
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
        let position = self
            .placed
            .binary_search_by_key(&node_id, |placed| placed.node_id)
            .expect("the placement covers every node the graph holds");
        NodeIdx(position as u32)
    }

    /// [`Self::resolve`] behind the type gate: a wire whose resolved source type
    /// is incompatible with the declared input, or a const that doesn't satisfy
    /// it, lowers as unbound — drift tolerance. The editor paints such a wire as
    /// mismatched; a required input surfaces as a missing-input verdict. Nothing
    /// is severed, so the wiring revives when the types line up again.
    fn typed_binding(
        &self,
        library: &Library,
        input: &FuncInput,
        binding: Option<&Binding>,
    ) -> ExecutionBinding {
        match binding {
            None => ExecutionBinding::None,
            Some(Binding::Const(value)) if library.const_satisfies(input, value) => {
                ExecutionBinding::Const(value.clone())
            }
            Some(Binding::Const(_)) => ExecutionBinding::None,
            Some(Binding::Bind(src)) => {
                // A port the table missed is one no chain reached *and* no
                // declaration names, and `Any` is what that resolved to before
                // the table existed: the gate passes and `resolve` unbinds it on
                // the range check. A port the table *did* stamp still has to
                // pass that check — a wildcard chain records every port it walks
                // through, out-of-range ones included, as `Any`.
                let resolved = self.output_types.get(*src).cloned().unwrap_or_default();
                if input.data_type.compatible_with(&resolved) {
                    self.resolve(*src)
                } else {
                    ExecutionBinding::None
                }
            }
        }
    }

    /// Intern an output reference into the dense [`OutputAddr`] the producer
    /// landed at — the one place a producer id is ever hashed, once per compile
    /// instead of once per run.
    ///
    /// Library drift can leave a binding to an output the func no longer
    /// declares — degrade to unbound rather than addressing a vanished slot
    /// (the planner reports the consumer's missing input). The count comes off
    /// the placement rather than the producer's emitted node, which the id order
    /// may not have reached yet. The *node* is never missing:
    /// `Graph::validate_shape` rejects a binding naming a producer the graph
    /// does not hold, and the placement covers every node it does.
    fn resolve(&self, port: OutputPort) -> ExecutionBinding {
        let OutputPort { node_id, port_idx } = port;
        let node_idx = self.idx(node_id);
        if port_idx >= self.placed[node_idx.0 as usize].outputs as usize {
            return ExecutionBinding::None;
        }
        ExecutionBinding::Bind(OutputAddr {
            node_idx,
            port_idx: port_idx as u32,
        })
    }

    /// Give each placed event port the subscribers this graph wires to it.
    ///
    /// Run after the walk because a subscriber's slot belongs to the *emitter*,
    /// which the id order may put after the subscriber. Nothing outside this
    /// call can see the column part-wired: it is still a local of
    /// [`Self::walk`], and every list is complete before it moves into the
    /// artifact.
    ///
    /// A disabled node fires no events and receives none, and a subscription
    /// past the emitter's run — an event the func has since dropped — wires
    /// nothing: the same drift tolerance the type gate applies to data edges.
    ///
    /// Both verdicts come off the *placed* node rather than the authoring one.
    /// The walk copied `disabled` onto every node it emitted and claimed the run
    /// there too, so `graph` is read for the subscriptions alone — one placement
    /// lookup per endpoint answers everything else, where going back to the
    /// graph would hash each id a second time for a flag already in hand.
    ///
    /// Takes no compiler state: the placement answers for identity and the two
    /// columns are the walk's own.
    fn wire_subscriptions(&self, graph: &Graph, compiled: &mut CompiledGraph) {
        for sub in graph.subscriptions() {
            let emitter_idx = self.idx(sub.emitter);
            let subscriber_idx = self.idx(sub.subscriber);
            if compiled.e_nodes[emitter_idx].disabled || compiled.e_nodes[subscriber_idx].disabled {
                continue;
            }
            let run = compiled.e_nodes[emitter_idx].events;
            if sub.event_idx >= run.len as usize {
                continue;
            }
            compiled.events[run.nth(sub.event_idx as u32)]
                .subscribers
                .push(subscriber_idx);
        }
    }
}

#[cfg(any(test, feature = "internals"))]
pub(crate) mod internals {
    use std::sync::Arc;

    use crate::execution::compile::compiled_graph::{CompiledGraph, ExecutionNode};
    use crate::graph::identity::NodeId;

    /// A [`CompiledGraph`] of bare nodes, for a host test that only has to
    /// resolve authored ids against a program.
    #[derive(Debug, Default)]
    pub struct CompiledGraphBuilder {
        node_ids: Vec<NodeId>,
    }

    impl CompiledGraphBuilder {
        pub fn new() -> Self {
            Self::default()
        }

        /// Add the execution node an authored node became. One node per authored
        /// id now that nothing dissolves, so the two are the same identity.
        pub fn insert_node(&mut self, node_id: NodeId) {
            self.node_ids.push(node_id);
        }

        /// Sorted on the way in, like the real walk: a fixture that placed its
        /// nodes in insertion order would let a test pass against an index
        /// layout no compile produces.
        pub fn build(mut self) -> Arc<CompiledGraph> {
            self.node_ids.sort();
            let mut compiled = CompiledGraph::default();
            for node_id in self.node_ids {
                // These nodes declare no ports, so the default is exactly the
                // bare node a fixture wants.
                compiled.push(node_id, ExecutionNode::default());
            }
            Arc::new(compiled)
        }
    }
}

#[cfg(test)]
mod tests;
