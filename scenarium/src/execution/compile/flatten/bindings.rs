//! Data-binding routing across composite boundaries.

use super::Run;
use crate::DataType;
use crate::execution::compile::flat::FlatBinding;
use crate::execution::identity::ExecutionOutputPort;
use crate::graph::definition::GraphDef;
use crate::graph::func::FuncInput;
use crate::graph::identity::{InputPort, NodeId, OutputPort};
use crate::graph::node::NodeKind;
use crate::graph::{Binding, Graph, NodeSearch};

impl<'a> Run<'a> {
    /// Resolve an output reference in the current frame to a concrete flat
    /// producer, following through boundary and composite nodes. Leaves the
    /// descent stack as it found it.
    fn resolve(&mut self, port: OutputPort) -> FlatBinding {
        let OutputPort { node_id, port_idx } = port;
        let graph = self.current();
        let node = graph
            .find(node_id, NodeSearch::TopLevel)
            .expect("binding to a missing node");
        match &node.kind {
            NodeKind::Func(_) | NodeKind::Special(_) => {
                // Library drift can leave a binding to an output the func
                // no longer declares — degrade to unbound rather than
                // addressing a vanished slot (the planner reports the
                // consumer's missing input).
                if graph
                    .node_ports(node, self.library)
                    .is_some_and(|ports| port_idx >= ports.outputs.len())
                {
                    return FlatBinding::None;
                }
                FlatBinding::Bind(ExecutionOutputPort {
                    e_node_id: self.execution_node_id(node_id),
                    port_idx,
                })
            }
            // Follow into the composite: its output `port_idx` is wired by the
            // GraphOutput node's input `port_idx`.
            NodeKind::Graph(r) => {
                let nested = graph
                    .resolve_graph(*r, self.library)
                    .expect("graph node references a missing graph");
                self.push_level(node_id, &nested.body);
                let source = self.resolve_exposed_output(nested, port_idx);
                self.pop_level();
                source
            }
            // Follow out: this GraphInput output `port_idx` is the enclosing
            // instance's exposed input `port_idx`; resolve it one level up.
            NodeKind::GraphInput => {
                let instance_id = *self.path.last().expect("GraphInput at the root level");
                self.pop_level();
                let outer = self.current();
                let binding = outer.bindings.get(&InputPort::new(instance_id, port_idx));
                // The level below was descended through this very node, so
                // it is here and it is a graph — only the port may have
                // gone, which is drift.
                let instance = outer
                    .find(instance_id, NodeSearch::TopLevel)
                    .expect("the enclosing level was descended through this instance");
                let NodeKind::Graph(link) = &instance.kind else {
                    panic!("only a graph node opens a level");
                };
                let def = outer
                    .resolve_graph(*link, self.library)
                    .expect("graph node references a missing graph");
                // The instance's own interface declares this port, so the
                // exterior wire is gated against it — the same `FuncInput`
                // the interior side would be gated against, picker list and
                // optionality included.
                let source = match def.inputs.get(port_idx) {
                    Some(declared) => self.typed_binding(outer, declared, binding),
                    None => FlatBinding::None,
                };
                self.push_level(instance_id, graph);
                source
            }
            NodeKind::GraphOutput => FlatBinding::None,
        }
    }

    /// [`Self::resolve_binding`] behind the type gate: a wire whose resolved
    /// source type is incompatible with the declared input, or a const that
    /// doesn't satisfy it, flattens as unbound — the type half of drift
    /// tolerance. The editor paints such a wire as mismatched; a required
    /// input surfaces as a missing-input verdict. Nothing is severed, so the
    /// wiring revives when the types line up again.
    pub(super) fn typed_binding(
        &mut self,
        graph: &'a Graph,
        input: &FuncInput,
        binding: Option<&Binding>,
    ) -> FlatBinding {
        let mismatched = match binding {
            Some(Binding::Bind(src)) => !input
                .data_type
                .compatible_with(&graph.resolve_output_type(self.library, *src)),
            Some(Binding::Const(value)) => !self.library.const_satisfies(input, value),
            None => false,
        };
        if mismatched {
            return FlatBinding::None;
        }
        self.resolve_binding(binding)
    }

    /// Resolve `nested`'s interface output `port_idx` through the interior
    /// wiring behind it, gated by the declared port type. Call with
    /// `nested`'s own level pushed.
    ///
    /// The single definition of "what comes out of this port", shared by
    /// the on-demand hop in [`Self::resolve`] and the eager record in
    /// [`Self::record_exposed_outputs`] — the two must agree, or an
    /// instance would name a producer its consumers never see.
    fn resolve_exposed_output(&mut self, nested: &'a GraphDef, port_idx: usize) -> FlatBinding {
        // Drift can leave the definition without a boundary node, or the
        // interface without this port at all.
        let (Some(output_node), Some(declared)) = (
            nested.body.boundary_node(NodeKind::GraphOutput),
            nested.outputs.get(port_idx),
        ) else {
            return FlatBinding::None;
        };
        let declared = declared.ty.declared();
        let binding = nested
            .body
            .bindings
            .get(&InputPort::new(output_node, port_idx));
        self.typed_boundary_binding(&nested.body, &declared, binding)
    }

    /// Note which execution node backs each of `nested`'s interface output
    /// ports, against the instance `instance_id`. Called with that
    /// instance's level already pushed.
    ///
    /// Eager, and for every instance — not only those something binds to.
    /// The finished program has no `GraphOutput` edges left, so nothing
    /// downstream can tell an exposed producer from interior plumbing:
    /// with an interior reader and no exterior one, its consumer set is
    /// entirely inside the footprint, exactly like a node the instance
    /// merely uses. That is the shape `run_targets` used to skip.
    pub(super) fn record_exposed_outputs(&mut self, instance_id: NodeId, nested: &'a GraphDef) {
        for port_idx in 0..nested.outputs.len() {
            if let FlatBinding::Bind(port) = self.resolve_exposed_output(nested, port_idx) {
                self.flat.exposed.push((instance_id, port.e_node_id));
            }
        }
    }

    /// [`Self::typed_binding`] for a **boundary** port, whose declaration is
    /// a bare [`DataType`] rather than a `FuncInput`.
    ///
    /// A composite's interface is a type contract in its own right. The
    /// outer gate checks the consumer against the *declared* port type and
    /// passes; crossing the boundary through raw `resolve_binding` then
    /// ignored that declaration entirely, so a definition declaring an
    /// `Int` output while wiring it to a `String` producer compiled a
    /// direct `Bind` from that producer into an `Int` consumer. Nothing
    /// downstream could see it — the edge that would have shown the
    /// mismatch is exactly the one flattening dissolves.
    fn typed_boundary_binding(
        &mut self,
        graph: &'a Graph,
        declared: &DataType,
        binding: Option<&Binding>,
    ) -> FlatBinding {
        let mismatched = match binding {
            Some(Binding::Bind(src)) => {
                !declared.compatible_with(&graph.resolve_output_type(self.library, *src))
            }
            Some(Binding::Const(value)) => !self.library.declared_accepts_const(declared, value),
            None => false,
        };
        if mismatched {
            return FlatBinding::None;
        }
        self.resolve_binding(binding)
    }

    fn resolve_binding(&mut self, binding: Option<&Binding>) -> FlatBinding {
        match binding {
            None => FlatBinding::None,
            Some(Binding::Const(value)) => FlatBinding::Const(value.clone()),
            Some(Binding::Bind(output)) => self.resolve(*output),
        }
    }
}
