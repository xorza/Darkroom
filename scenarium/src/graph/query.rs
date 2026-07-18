//! `Graph` port-type resolution: declared input/output types, wildcard
//! reroute following, and the edge-invalidation walk. Editor-only — the
//! engine never type-checks.

use hashbrown::HashSet;

use crate::graph::{Binding, Graph, InputPort, Node, NodeId, NodeKind, NodeSearch, OutputPort};
use crate::library::Library;
use crate::node::definition::{FuncInput, FuncOutput, OutputType};
use crate::{DataType, StaticValue, closes_data_cycle};

impl Graph {
    /// The declared type of input `port`, or `None` when it can't be resolved —
    /// a boundary node (its arity mirrors the enclosing interface, with no
    /// per-port type here) or a missing func/graph. The caller treats `None` as
    /// the polymorphic `Any`. The engine never type-checks, so only the editor
    /// calls this.
    pub fn input_type(&self, library: &Library, port: InputPort) -> Option<DataType> {
        self.input_spec(library, port).map(|i| i.data_type.clone())
    }

    /// The declared [`FuncInput`] of input `port` — its full spec (type +
    /// `value_variants` + flags), or `None` for a boundary / unresolved node.
    /// Resolution mirrors [`Self::input_type`].
    pub(crate) fn input_spec<'a>(
        &'a self,
        library: &'a Library,
        port: InputPort,
    ) -> Option<&'a FuncInput> {
        let node = self.find(&port.node_id, NodeSearch::TopLevel)?;
        let inputs = match &node.kind {
            NodeKind::Func(func_id) => &library.by_id(func_id)?.inputs,
            NodeKind::Graph(r) => &self.resolve_graph(*r, library)?.inputs,
            NodeKind::Special(s) => &s.func().inputs,
            NodeKind::GraphInput | NodeKind::GraphOutput => return None,
        };
        inputs.get(port.port_idx)
    }

    /// The effective type produced at output `port`, following *wildcard*
    /// outputs up the graph: a wildcard output (e.g. a passthrough / reroute —
    /// see [`OutputType::Wildcard`](crate::node::definition::OutputType))
    /// reports the resolved type of whatever feeds the input it mirrors, so a
    /// value's type survives the hop: a `Bind` follows the producer up, while a
    /// `Const` takes the mirrored input's declared type (which carries the full
    /// `FsPathConfig` / `Enum` id), or the const's own scalar type on a wildcard
    /// (`Any`-declared) input. The walk dead-ends polymorphic (`Any`) when the
    /// mirrored input is unbound, the producer is a boundary or missing, or a
    /// binding cycle is hit. Used by the editor for port-type display, connection
    /// compatibility, and graph-interface inference; the engine never
    /// type-checks, so it never calls this.
    pub fn resolve_output_type(&self, library: &Library, port: OutputPort) -> DataType {
        self.resolve_output_type_inner(library, port, &mut HashSet::new())
    }

    fn resolve_output_type_inner(
        &self,
        library: &Library,
        port: OutputPort,
        visiting: &mut HashSet<NodeId>,
    ) -> DataType {
        let Some(out) = self.output_spec(library, port) else {
            return DataType::Any;
        };

        // A fixed output is just its declared type; a wildcard reports the type
        // of whatever feeds the input it mirrors (any func/special node can
        // declare one).
        let OutputType::Wildcard { mirrors } = &out.ty else {
            return out.ty.declared();
        };
        let mirrors = *mirrors;

        // A binding cycle can momentarily exist in the editor graph (flatten and
        // the planner reject it later) — break it as polymorphic rather than
        // recurse forever.
        if !visiting.insert(port.node_id) {
            return DataType::Any;
        }
        let mirror = InputPort::new(port.node_id, mirrors);
        let resolved = match self.input_binding(mirror) {
            Binding::Bind(src) => self.resolve_output_type_inner(library, src, visiting),
            // The mirrored input's *declared* type already carries the full
            // `FsPathConfig` / `Enum` id (and a `Custom` id for a preset port), so
            // a const of any kind resolves to it. A wildcard (`Any`-declared)
            // input instead takes the const's own, more specific scalar type; a
            // bare path/enum/null const there isn't reconstructable from the
            // value alone, so it stays polymorphic (`Any`).
            Binding::Const(v) => match self.input_type(library, mirror) {
                Some(declared) if !matches!(declared, DataType::Any) => declared,
                _ => match v {
                    StaticValue::Float(_) => DataType::Float,
                    StaticValue::Int(_) => DataType::Int,
                    StaticValue::Bool(_) => DataType::Bool,
                    StaticValue::String(_) => DataType::String,
                    StaticValue::Null | StaticValue::FsPath(_) | StaticValue::Enum(_) => {
                        DataType::Any
                    }
                },
            },
            // Nothing wired in yet — polymorphic.
            Binding::None => DataType::Any,
        };
        visiting.remove(&port.node_id);
        resolved
    }

    /// The output ports `node` declares (func / special spec, or graph
    /// graph interface), or `None` for a boundary / unresolved node.
    fn node_outputs<'a>(
        &'a self,
        library: &'a Library,
        node: &'a Node,
    ) -> Option<&'a [FuncOutput]> {
        match &node.kind {
            NodeKind::Func(func_id) => library.by_id(func_id).map(|f| f.outputs.as_slice()),
            NodeKind::Graph(r) => self.resolve_graph(*r, library).map(|d| d.outputs.as_slice()),
            NodeKind::Special(s) => Some(s.func().outputs.as_slice()),
            NodeKind::GraphInput | NodeKind::GraphOutput => None,
        }
    }

    /// The declared [`FuncOutput`] of output `port` — its name + [`OutputType`],
    /// or `None` for a boundary / unresolved node. The output-side mirror of
    /// [`Self::input_spec`].
    fn output_spec<'a>(&'a self, library: &'a Library, port: OutputPort) -> Option<&'a FuncOutput> {
        let node = self.find(&port.node_id, NodeSearch::TopLevel)?;
        self.node_outputs(library, node)?.get(port.port_idx)
    }

    /// Output ports of `input.node_id` whose type *mirrors* `input` —
    /// wildcard (passthrough / reroute) outputs that retype when that input
    /// changes. Empty for an ordinary input or node.
    fn wildcard_outputs_mirroring(&self, library: &Library, input: InputPort) -> Vec<OutputPort> {
        let Some(node) = self.find(&input.node_id, NodeSearch::TopLevel) else {
            return Vec::new();
        };
        let Some(outputs) = self.node_outputs(library, node) else {
            return Vec::new();
        };
        outputs
            .iter()
            .enumerate()
            .filter(|(_, o)| {
                matches!(o.ty, OutputType::Wildcard { mirrors } if mirrors == input.port_idx)
            })
            .map(|(i, _)| OutputPort::new(input.node_id, i))
            .collect()
    }

    /// The input ports whose incoming `Bind` is no longer type-compatible once
    /// `changed` changed — the wires the editor drops to keep the graph
    /// well-typed. Follows wildcard outputs *transitively*: a passthrough /
    /// reroute fed by the change retypes too, so its now-incompatible consumers
    /// (any number of hops downstream) are included. Empty when the change
    /// retypes nothing (an ordinary node's input, or a wildcard input no output
    /// mirrors). The engine never type-checks, so only the editor calls this.
    pub fn edges_invalidated_by(&self, library: &Library, changed: InputPort) -> Vec<InputPort> {
        // Output ports whose resolved type just changed, grown forward from
        // `changed`'s wildcard outputs through downstream wildcards. The
        // set doubles as the cycle / re-visit guard.
        let mut retyped: HashSet<OutputPort> = HashSet::new();
        let mut frontier: Vec<OutputPort> = Vec::new();
        for port in self.wildcard_outputs_mirroring(library, changed) {
            if retyped.insert(port) {
                frontier.push(port);
            }
        }

        let mut invalidated = Vec::new();
        while let Some(src) = frontier.pop() {
            let source_ty = self.resolve_output_type(library, src);
            for (dst, edge_src) in self.edges() {
                if edge_src != src {
                    continue;
                }
                match self.input_type(library, dst) {
                    // The consumer can't accept the retyped value — drop the wire.
                    Some(sink_ty) if !sink_ty.compatible_with(&source_ty) => invalidated.push(dst),
                    // Kept: a wildcard output of the consumer mirroring this
                    // input retypes too, so follow it further downstream.
                    _ => {
                        for port in self.wildcard_outputs_mirroring(library, dst) {
                            if retyped.insert(port) {
                                frontier.push(port);
                            }
                        }
                    }
                }
            }
        }
        invalidated
    }

    /// Every data edge as (consumer input ← producer output). Const bindings
    /// are not edges and are skipped.
    pub fn edges(&self) -> impl Iterator<Item = (InputPort, OutputPort)> + '_ {
        self.bindings
            .iter()
            .filter_map(|(dst, binding)| match binding {
                Binding::Bind(src) => Some((*dst, *src)),
                _ => None,
            })
    }

    /// Whether binding `consumer`'s input to `producer`'s output would close a
    /// directed data cycle. Convenience wrapper over [`closes_data_cycle`] for
    /// callers holding a `Graph` (the editor's intent layer, which rejects such
    /// a bind before it commits). The planner remains the authoritative
    /// backstop (`Error::CycleDetected`).
    pub fn would_create_cycle(&self, producer: NodeId, consumer: NodeId) -> bool {
        closes_data_cycle(
            self.edges().map(|(dst, src)| (src.node_id, dst.node_id)),
            producer,
            consumer,
        )
    }
}
