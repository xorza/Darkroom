//! Phase 1 of the pipeline, split off the engine so hosts compile on their own
//! thread: validate the authoring [`Graph`] against the [`Library`], then walk
//! it straight into a self-contained [`CompiledGraph`] the worker installs
//! as-is. Compile errors surface synchronously at the call site — a graph that
//! doesn't compile is never sent, so the worker's install is infallible and a
//! running event loop is never disturbed by a bad edit.
//!
//! A graph is flat, so the walk is a copy with four jobs it is the only place to
//! do: resolving each binding to the producer it names, gating it against the
//! declared types, stamping each output with the effective type the same pass
//! resolved, and interning every id-named reference into the dense index space.
//! The *type gate* is drift tolerance — a wire whose resolved source type no
//! longer fits the consumer, or a const that no longer satisfies it, lowers as
//! unbound rather than severing authored wiring.
//!
//! **The sort comes first.** [`Graph::iter`] is a `HashMap` walk, so the order
//! nodes are reached in is not an order an artifact may depend on. Sorting the
//! ids *before* emitting anything settles the whole dense index space up front:
//! a node's `NodeIdx` is its position in that sort, so a binding can name its
//! producer's index the moment the walk resolves it, and no later pass has to
//! revisit a column to translate one. This is where the crate's two identity
//! spaces meet, and it crosses in one direction only — stable
//! [`NodeId`]/[`OutputPort`] in, dense `NodeIdx`/`OutputAddr` out.
//!
//! Everything the walk produces is final. The program is immutable for the life
//! of the install — no later pass fills a field in — so each step below produces
//! its part complete rather than reserving space for it.
//!
//! See `README.md` Part A §5.

pub(crate) mod error;
mod validate;

use hashbrown::HashMap;

use crate::DataType;
use crate::common::column::{Column, Span};
use crate::execution::compile::error::CompileError;
use crate::execution::compiled::{
    CompiledGraph, ExecutionBinding, ExecutionEvent, ExecutionInput, ExecutionNode,
};
use crate::execution::identity::{EventIdx, NodeIdx, OutputAddr};
use crate::graph::func::event::EventLambda;
use crate::graph::func::{Func, FuncInput};
use crate::graph::identity::{InputPort, NodeId, OutputPort};
use crate::graph::node::NodeKind;
use crate::graph::node::special::SpecialNode;
use crate::graph::output_types::OutputTypes;
use crate::graph::{Binding, Graph};
use crate::library::Library;

/// One node's place in the dense space, settled before the walk so a reference
/// naming it resolves on the spot — including a reference the walk meets before
/// it reaches the node itself.
#[derive(Debug)]
struct Placed {
    node_id: NodeId,
    /// The node's declared output count. A binding is range-checked against it,
    /// and the producer's own `outputs` run does not exist until the walk emits
    /// that node — which may be after the consumer naming it.
    outputs: u32,
}

/// The compile entry point, owning every buffer the walk would otherwise
/// allocate. Only the walk's own scratch lives here; everything it *produces*
/// goes into the columns that become the artifact, so no part of one compile
/// survives into the next.
///
/// Hosts keep one per compile site (e.g. darkroom's `Engine`), so an editor that
/// recompiles per edit pays for the artifact and nothing else. The produced
/// [`CompiledGraph`] is the one thing here that cannot be reused, since the
/// engine, its runtime cache, and the GUI all hold handles to the previous one
/// while the next compile runs — so it is always fresh and can be shared with
/// the worker in an [`Arc`](std::sync::Arc).
#[derive(Debug, Default)]
pub struct Compiler {
    /// Every node, in id order — the dense index space. A node's `NodeIdx` is
    /// its position here.
    placed: Vec<Placed>,
    /// One node's resolved inputs, refilled per node. They are resolved before
    /// the ports are appended, so an [`ExecutionInput`] is whole the moment it
    /// exists.
    node_inputs: Vec<ExecutionInput>,
    /// Each event port's lambda, in pool order, collected as the walk passes the
    /// node that declares it — the one place both the pool position and the
    /// declaration are in hand at once.
    event_lambdas: Vec<EventLambda>,
    /// Each event's subscribers, grouped before the events are built. Only the
    /// outer vector is reused: the inner ones move into the events.
    subscribers: Vec<Vec<NodeIdx>>,
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
    pub fn compile(
        &mut self,
        graph: &Graph,
        library: &Library,
    ) -> Result<CompiledGraph, CompileError> {
        // Validate before building anything: the graph+library pair is untrusted
        // input, and a passing check lets the walk below resolve every reference
        // infallibly.
        if let Err(e) = graph.validate_with(library) {
            tracing::error!(error = %e, "compile rejected: invalid graph");
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
    /// The columns built below are locals rather than fields: they do not
    /// survive the call as buffers — they *become* the program, assembled in one
    /// move at the end, so nothing ever observes a half-built [`CompiledGraph`].
    /// Only what stays behind belongs on the compiler.
    ///
    /// `library` is a compile-time input like the graph beside it: every port,
    /// lambda, and flag a node needs is copied out here (the lambdas are `Arc`s,
    /// so a copy is a refcount bump), which is what leaves the artifact
    /// self-contained.
    fn walk(&mut self, root: &Graph, library: &Library) -> CompiledGraph {
        self.output_types.update(root, library);
        let node_index = self.place_nodes(root, library);

        let mut node_ids = Column::default();
        let mut e_nodes = Column::default();
        let mut inputs = Column::default();
        let mut outputs = Column::default();
        self.event_lambdas.clear();

        for position in 0..self.placed.len() {
            let node_id = self.placed[position].node_id;
            let node = root
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
            // Bindings are resolved *before* the ports are appended, so each
            // input is whole when it enters the pool rather than being revisited
            // by index afterwards.
            self.node_inputs.clear();
            for (port_idx, func_input) in func.inputs.iter().enumerate() {
                let port = InputPort::new(node_id, port_idx);
                let binding =
                    self.typed_binding(library, &node_index, func_input, root.bindings.get(&port));
                self.node_inputs.push(ExecutionInput {
                    required: func_input.required,
                    stamps_fs_path: matches!(&func_input.data_type, DataType::FsPath(_)),
                    binding,
                });
            }
            let node_inputs = inputs.append(self.node_inputs.drain(..));

            // The effective type of each output, straight off the table filled
            // above — the same answer the editor paints, so the program and the
            // canvas agree about a wildcard by construction rather than by two
            // walks happening to match. A port the table missed is library
            // drift, and `Any` is what that resolved to before it existed.
            let node_outputs = outputs.append((0..func.outputs.len()).map(|port_idx| {
                self.output_types
                    .get(OutputPort::new(node_id, port_idx))
                    .cloned()
                    .unwrap_or_default()
            }));

            // The event pool is claimed here and filled by `wire_subscriptions`
            // below: a subscriber's slot belongs to the *emitter*, which the
            // walk may not have reached yet. The lambda is the one half the
            // declaration answers for, so it rides along now.
            let events = Span::new(
                u32::try_from(self.event_lambdas.len())
                    .expect("a program's port count fits in u32"),
                u32::try_from(func.events.len()).expect("a node's port count fits in u32"),
            );
            self.event_lambdas
                .extend(func.events.iter().map(|event| event.event_lambda.clone()));

            node_ids.push(node_id);
            e_nodes.push(ExecutionNode {
                sink: func.sink,
                disabled: node.disabled,
                behavior: func.behavior,
                cache: node.cache,
                special,
                inputs: node_inputs,
                outputs: node_outputs,
                events,
                func_id: func.id,
                lambda: func.lambda.clone(),
            });
            debug_assert_eq!(
                node_index[&node_id],
                NodeIdx(position as u32),
                "the walk emits in the order the placement assigned"
            );
        }

        let events = self.wire_subscriptions(root, &e_nodes, &node_index);

        CompiledGraph {
            e_nodes,
            node_ids,
            node_index,
            inputs,
            outputs,
            events,
        }
    }

    /// Settle the dense index space: every node in id order, its declared output
    /// count beside it, and the reverse map the walk resolves references
    /// against.
    ///
    /// Id order rather than `Graph::iter`'s, so the artifact is deterministic
    /// however the walk happens to reach the nodes. Ids come from a map keyed by
    /// them, so the sort cannot produce a duplicate.
    ///
    /// The output counts are taken here rather than during the walk because a
    /// wire is range-checked against its *producer*, which the id order may put
    /// after the consumer that names it.
    fn place_nodes(&mut self, root: &Graph, library: &Library) -> HashMap<NodeId, NodeIdx> {
        self.placed.clear();
        self.placed.extend(root.iter().map(|node| {
            Placed {
                node_id: node.id,
                outputs: root
                    .node_func(&node, library)
                    .expect("func resolved by validate_with")
                    .outputs
                    .len() as u32,
            }
        }));
        assert!(
            u32::try_from(self.placed.len()).is_ok(),
            "program node count must fit in u32"
        );
        self.placed.sort_unstable_by_key(|placed| placed.node_id);

        let mut node_index = HashMap::with_capacity(self.placed.len());
        for (position, placed) in self.placed.iter().enumerate() {
            node_index.insert(placed.node_id, NodeIdx(position as u32));
        }
        node_index
    }

    /// [`Self::resolve`] behind the type gate: a wire whose resolved source type
    /// is incompatible with the declared input, or a const that doesn't satisfy
    /// it, lowers as unbound — drift tolerance. The editor paints such a wire as
    /// mismatched; a required input surfaces as a missing-input verdict. Nothing
    /// is severed, so the wiring revives when the types line up again.
    fn typed_binding(
        &self,
        library: &Library,
        node_index: &HashMap<NodeId, NodeIdx>,
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
                    self.resolve(node_index, *src)
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
    fn resolve(&self, node_index: &HashMap<NodeId, NodeIdx>, port: OutputPort) -> ExecutionBinding {
        let OutputPort { node_id, port_idx } = port;
        let node_idx = node_index[&node_id];
        if port_idx >= self.placed[node_idx.0 as usize].outputs as usize {
            return ExecutionBinding::None;
        }
        ExecutionBinding::Bind(OutputAddr {
            node_idx,
            port_idx: port_idx as u32,
        })
    }

    /// Build the event pool: each port's declared lambda, plus the subscribers
    /// this graph wires to it.
    ///
    /// Run after the walk because a subscriber's slot belongs to the emitter,
    /// whose run the walk may not have claimed yet. Subscribers are grouped
    /// before the events are built, so each one is whole when it enters the pool
    /// — an empty subscriber list then means "nothing subscribes" rather than
    /// "not wired yet".
    ///
    /// A disabled node fires no events and receives none, and a subscription
    /// past the emitter's run — an event the func has since dropped — wires
    /// nothing: the same drift tolerance the type gate applies to data edges.
    /// The run is read off the placed node rather than the declaration, so the
    /// bound is the one the walk actually claimed.
    fn wire_subscriptions(
        &mut self,
        graph: &Graph,
        e_nodes: &Column<NodeIdx, ExecutionNode>,
        node_index: &HashMap<NodeId, NodeIdx>,
    ) -> Column<EventIdx, ExecutionEvent> {
        self.subscribers.clear();
        self.subscribers
            .resize_with(self.event_lambdas.len(), Vec::new);
        for sub in graph.subscriptions() {
            let emitter = graph
                .find(sub.emitter)
                .expect("subscription emitter resolved by validate_with");
            let subscriber = graph
                .find(sub.subscriber)
                .expect("subscriber resolved by validate_with");
            if emitter.disabled || subscriber.disabled {
                continue;
            }
            let events = e_nodes[node_index[&sub.emitter]].events;
            if sub.event_idx >= events.len as usize {
                continue;
            }
            self.subscribers[events.start as usize + sub.event_idx]
                .push(node_index[&sub.subscriber]);
        }

        let mut events = Column::default();
        events.append(
            self.subscribers
                .drain(..)
                .zip(self.event_lambdas.drain(..))
                .map(|(subscribers, lambda)| ExecutionEvent {
                    subscribers,
                    lambda,
                }),
        );
        events
    }
}

#[cfg(any(test, feature = "internals"))]
pub(crate) mod internals {
    use std::sync::Arc;

    use crate::execution::compiled::{CompiledGraph, ExecutionNode};
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
