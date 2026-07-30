//! Graph flattening: the pipeline's first stage. Walks the authoring `Graph`
//! and appends every node straight into the [`FlatGraph`] it fills — no
//! intermediate `Graph` is materialized and no final
//! [`CompiledGraph`](crate::execution::compiled::CompiledGraph) is touched.
//!
//! A graph is flat, so "flattening" is a copy with three jobs the walk is the
//! only place to do: resolving each binding to the producer it names, gating it
//! against the declared types, and stamping each output with the effective type
//! the same pass resolved. The *type gate* is drift tolerance — a wire whose
//! resolved source type no longer fits the consumer, or a const that no longer
//! satisfies it, flattens as unbound rather than severing authored wiring.
//!
//! Everything here is in the **stable-id space**: the walk names producers,
//! subscribers, and emitters by [`ExecutionNodeId`] because a node's dense
//! index is its position after the id sort, which is linking's to assign. So
//! nothing here mentions `NodeIdx` or `OutputAddr`: the walk emits program
//! [`ExecutionNode`]s directly, but the columns it appends them to stay in emit
//! order.
//!
//! See `README.md` Part A §5.

pub(crate) mod flat;

use crate::DataType;
use crate::execution::compiled::ExecutionNode;
use crate::execution::flatten::flat::{FlatBinding, FlatGraph, FlatInput, PendingSubscription};
use crate::execution::identity::{ExecutionEventPort, ExecutionNodeId, ExecutionOutputPort};
use crate::graph::func::{Func, FuncInput};
use crate::graph::identity::{InputPort, OutputPort};
use crate::graph::node::NodeKind;
use crate::graph::node::special::SpecialNode;
use crate::graph::output_types::OutputTypes;
use crate::graph::{Binding, Graph};
use crate::library::Library;

/// Reusable traversal scratch owned by the
/// [`Compiler`](crate::execution::compile::Compiler). Only the walk's own
/// state lives here; everything it *produces* goes straight into the
/// [`FlatGraph`] it fills, so no part of one flatten survives into the next.
#[derive(Debug, Default)]
pub(crate) struct Flattener {
    /// One node's resolved inputs, refilled per node. They are resolved before
    /// the ports are appended, so a [`FlatInput`] is whole the moment it exists.
    node_inputs: Vec<FlatInput>,
    /// Every output type of the graph, filled before the walk. Held here
    /// because the type gate runs once per bound input, and resolving one port
    /// at a time cost a walk per edge.
    output_types: OutputTypes,
}

impl Flattener {
    /// Lower `root` against `library` into `out`. One step, one value: the walk
    /// appends what it finds, and what `out` holds afterwards is complete — no
    /// field of it is filled in later and no final program or artifact type is
    /// touched on the way.
    ///
    /// `out` is the caller's buffer, emptied here rather than allocated, so a
    /// compile pays for the artifact alone. It is cleared on entry instead of
    /// trusted, which is what lets the same buffer serve every compile without
    /// the walk having to know whether the last one was linked.
    pub(crate) fn flatten(&mut self, root: &Graph, library: &Library, out: &mut FlatGraph) {
        self.node_inputs.clear();
        out.clear();
        self.output_types.update(root, library);

        for node in root.iter() {
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
                let port = InputPort::new(node.id, port_idx);
                let binding =
                    self.typed_binding(root, library, func_input, root.bindings.get(&port));
                self.node_inputs.push(FlatInput {
                    required: func_input.required,
                    stamps_fs_path: matches!(&func_input.data_type, DataType::FsPath(_)),
                    binding,
                });
            }
            let inputs = out.inputs.append(self.node_inputs.drain(..));

            // The effective type of each output, straight off the table filled
            // above — the same answer the editor paints, so the program and the
            // canvas agree about a wildcard by construction rather than by two
            // walks happening to match. A port the table missed is library
            // drift, and `Any` is what that resolved to before it existed.
            let outputs = out.outputs.append((0..func.outputs.len()).map(|port_idx| {
                self.output_types
                    .get(OutputPort::new(node.id, port_idx))
                    .cloned()
                    .unwrap_or_default()
            }));

            // Reserved, not emitted: an event's lambda is the library's to
            // state and linking holds the library, so the walk settles only the
            // arity that fixes the run.
            let events = out.reserve_events(func.events.len());

            out.push_node(
                ExecutionNodeId::from_node(node.id),
                ExecutionNode {
                    sink: func.sink,
                    disabled: node.disabled,
                    behavior: func.behavior,
                    cache: node.cache,
                    special,
                    inputs,
                    outputs,
                    events,
                    func_id: func.id,
                    lambda: func.lambda.clone(),
                },
            );
        }

        self.collect_subscriptions(root, library, out);
    }

    /// [`Self::resolve_binding`] behind the type gate: a wire whose resolved
    /// source type is incompatible with the declared input, or a const that
    /// doesn't satisfy it, flattens as unbound — drift tolerance. The editor
    /// paints such a wire as mismatched; a required input surfaces as a
    /// missing-input verdict. Nothing is severed, so the wiring revives when
    /// the types line up again.
    fn typed_binding(
        &self,
        graph: &Graph,
        library: &Library,
        input: &FuncInput,
        binding: Option<&Binding>,
    ) -> FlatBinding {
        let mismatched = match binding {
            Some(Binding::Bind(src)) => {
                // A miss is library drift — a binding naming an output the func
                // no longer declares — and `Any` is what that resolved to before
                // the table existed: the gate passes, and `resolve` below
                // degrades the edge to unbound.
                let resolved = self.output_types.get(*src).cloned().unwrap_or_default();
                !input.data_type.compatible_with(&resolved)
            }
            Some(Binding::Const(value)) => !library.const_satisfies(input, value),
            None => false,
        };
        if mismatched {
            return FlatBinding::None;
        }
        match binding {
            None => FlatBinding::None,
            Some(Binding::Const(value)) => FlatBinding::Const(value.clone()),
            Some(Binding::Bind(output)) => Self::resolve(graph, library, *output),
        }
    }

    /// Resolve an output reference to the flat producer it names.
    ///
    /// Library drift can leave a binding to an output the func no longer
    /// declares — degrade to unbound rather than addressing a vanished slot
    /// (the planner reports the consumer's missing input).
    fn resolve(graph: &Graph, library: &Library, port: OutputPort) -> FlatBinding {
        let OutputPort { node_id, port_idx } = port;
        let node = graph.find(node_id).expect("binding to a missing node");
        if graph
            .node_ports(node, library)
            .is_some_and(|ports| port_idx >= ports.outputs.len())
        {
            return FlatBinding::None;
        }
        FlatBinding::Bind(ExecutionOutputPort {
            e_node_id: ExecutionNodeId::from_node(node_id),
            port_idx,
        })
    }

    /// Turn this graph's event subscriptions into flat `(emitter event,
    /// subscriber)` edges. A disabled node fires no events and receives none.
    fn collect_subscriptions(&self, graph: &Graph, library: &Library, out: &mut FlatGraph) {
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
            // Drift tolerance: a subscription to an event the func no longer
            // declares wires nothing.
            if graph
                .node_ports(emitter, library)
                .is_some_and(|ports| sub.event_idx >= ports.events.len())
            {
                continue;
            }
            out.subscriptions.push(PendingSubscription {
                event: ExecutionEventPort {
                    e_node_id: ExecutionNodeId::from_node(sub.emitter),
                    event_idx: sub.event_idx,
                },
                subscriber: ExecutionNodeId::from_node(sub.subscriber),
            });
        }
    }
}

#[cfg(test)]
mod tests;
