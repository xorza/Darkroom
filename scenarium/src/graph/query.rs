//! `Graph` node-interface and port-type resolution: input/output/event arity,
//! declared types, wildcard reroute following, and edge invalidation. Shared
//! by compile-time validation and the editor.

use crate::graph::{Binding, Graph, InputPort, Node, NodeId, NodeKind, NodeSearch, OutputPort};
use crate::library::Library;
use crate::node::definition::{FuncInput, FuncOutput, OutputType};
use crate::node::output_type::{OutputTypeResolver, OutputTypeSource};
use crate::{DataType, closes_data_cycle};

impl Graph {
    /// The declared type of input `port`, or `None` when it can't be resolved —
    /// a boundary node (its arity mirrors the enclosing interface, with no
    /// per-port type here) or a missing func/graph. The caller treats `None` as
    /// the polymorphic `Any`.
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
            NodeKind::Graph(r) => &self.resolve_graph(*r, library)?.definition.as_ref()?.inputs,
            NodeKind::Special(s) => &s.func().inputs,
            NodeKind::GraphInput | NodeKind::GraphOutput => return None,
        };
        inputs.get(port.port_idx)
    }

    pub(crate) fn output_count(&self, node: &Node, library: &Library) -> Option<usize> {
        match &node.kind {
            NodeKind::Func(id) => library.by_id(id).map(|function| function.outputs.len()),
            NodeKind::Graph(reference) => self
                .resolve_graph(*reference, library)
                .and_then(|graph| graph.definition.as_ref())
                .map(|definition| definition.outputs.len()),
            NodeKind::Special(special) => Some(special.func().outputs.len()),
            NodeKind::GraphInput => self
                .definition
                .as_ref()
                .map(|definition| definition.inputs.len()),
            NodeKind::GraphOutput => Some(0),
        }
    }

    pub(crate) fn event_count(&self, node: &Node, library: &Library) -> Option<usize> {
        match &node.kind {
            NodeKind::Func(id) => library.by_id(id).map(|function| function.events.len()),
            NodeKind::Graph(reference) => self
                .resolve_graph(*reference, library)
                .and_then(|graph| graph.definition.as_ref())
                .map(|definition| definition.events.len()),
            NodeKind::Special(special) => Some(special.func().events.len()),
            NodeKind::GraphInput => self.definition.as_ref().map(|_| 1),
            NodeKind::GraphOutput => Some(0),
        }
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
    /// compatibility, graph-interface inference, and compile-time validation.
    pub fn resolve_output_type(&self, library: &Library, port: OutputPort) -> DataType {
        OutputTypeResolver::new(0).resolve(port, &|output| self.output_type_source(library, output))
    }

    fn output_type_source(
        &self,
        library: &Library,
        port: OutputPort,
    ) -> OutputTypeSource<OutputPort> {
        let Some(out) = self.output_spec(library, port) else {
            return OutputTypeSource::Unresolved;
        };
        let OutputType::Wildcard { mirrors } = &out.ty else {
            return OutputTypeSource::Fixed(out.ty.declared());
        };
        let mirror = InputPort::new(port.node_id, *mirrors);
        match self.bindings.get(&mirror) {
            Some(Binding::Bind(source)) => OutputTypeSource::Bind(*source),
            Some(Binding::Const(value)) => OutputTypeSource::Const {
                declared: self.input_type(library, mirror).unwrap_or_default(),
                value: value.clone(),
            },
            None => OutputTypeSource::Unresolved,
        }
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
            NodeKind::Graph(r) => self
                .resolve_graph(*r, library)
                .and_then(|graph| graph.definition.as_ref())
                .map(|definition| definition.outputs.as_slice()),
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
