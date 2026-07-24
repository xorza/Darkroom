//! `Graph` node-interface and port-type resolution: input/output/event arity,
//! declared types, wildcard reroute following, and edge invalidation. Shared
//! by compile-time validation and the editor.

use crate::graph::interface::GraphEvent;
use crate::graph::{
    Binding, Graph, GraphDef, InputPort, Node, NodeId, NodeKind, NodeSearch, OutputPort,
};
use crate::library::Library;
use crate::node::definition::{Func, FuncEvent, FuncInput, FuncOutput, OutputType};
use crate::node::output_type::{OutputTypeResolver, OutputTypeSource};
use crate::{DataType, closes_data_cycle};

/// The ports a node declares, borrowed from whatever declares them: a [`Func`]
/// in the library, a [`GraphDef`]'s interface, or a special node's hardcoded
/// spec. Resolved once by [`Graph::node_ports`] so no caller repeats the
/// per-kind lookup.
///
/// A boundary node has no ports of its own — its arity mirrors the enclosing
/// definition's interface — so it resolves to `None` rather than to a variant
/// here.
#[derive(Clone, Copy, Debug)]
pub struct NodePorts<'a> {
    pub name: &'a str,
    pub description: Option<&'a str>,
    pub inputs: &'a [FuncInput],
    pub outputs: &'a [FuncOutput],
    pub events: NodeEvents<'a>,
    /// The func this node instantiates, if any. Its declaration flags (sink,
    /// purity, cache policy) have no composite equivalent, so a caller that
    /// needs them decides for itself what a composite means.
    pub func: Option<&'a Func>,
}

/// A node's event ports. `FuncEvent` carries a lambda and `GraphEvent` an
/// interior emitter, so the two can't share a slice — only their names and
/// arity are common ground.
#[derive(Clone, Copy, Debug)]
pub enum NodeEvents<'a> {
    Func(&'a [FuncEvent]),
    Graph(&'a [GraphEvent]),
}

impl<'a> NodeEvents<'a> {
    pub fn len(self) -> usize {
        match self {
            NodeEvents::Func(events) => events.len(),
            NodeEvents::Graph(events) => events.len(),
        }
    }

    pub fn is_empty(self) -> bool {
        self.len() == 0
    }

    /// The event names in declaration order. One of the two slices is always
    /// empty, so the chain yields exactly the declared events.
    pub fn names(self) -> impl Iterator<Item = &'a str> {
        let func: &[FuncEvent] = match self {
            NodeEvents::Func(events) => events,
            NodeEvents::Graph(_) => &[],
        };
        let graph: &[GraphEvent] = match self {
            NodeEvents::Graph(events) => events,
            NodeEvents::Func(_) => &[],
        };
        func.iter()
            .map(|event| event.name.as_str())
            .chain(graph.iter().map(|event| event.name.as_str()))
    }
}

impl NodePorts<'_> {
    /// The const bindings a fresh instance starts with: one per input that
    /// declares a default, at that input's port index.
    pub fn default_bindings(self, node_id: NodeId) -> impl Iterator<Item = (InputPort, Binding)> {
        self.inputs
            .iter()
            .enumerate()
            .filter_map(move |(port_idx, input)| {
                let default = input.default_value.clone()?;
                Some((InputPort::new(node_id, port_idx), Binding::Const(default)))
            })
    }
}

impl<'a> From<&'a Func> for NodePorts<'a> {
    fn from(func: &'a Func) -> Self {
        NodePorts {
            name: &func.name,
            description: func.description.as_deref(),
            inputs: &func.inputs,
            outputs: &func.outputs,
            events: NodeEvents::Func(&func.events),
            func: Some(func),
        }
    }
}

impl GraphDef {
    /// This definition's interface as instance ports — what a node linking to
    /// it declares.
    pub fn ports(&self) -> NodePorts<'_> {
        NodePorts {
            name: &self.definition.name,
            description: None,
            inputs: &self.definition.inputs,
            outputs: &self.definition.outputs,
            events: NodeEvents::Graph(&self.definition.events),
            func: None,
        }
    }
}

impl Graph {
    /// The ports `node` declares, or `None` for a boundary node (its arity
    /// mirrors the enclosing interface, which a bare body doesn't carry) or an
    /// unresolved func/graph reference. The one place a node kind is mapped to
    /// its declaration.
    pub fn node_ports<'a>(&'a self, node: &'a Node, library: &'a Library) -> Option<NodePorts<'a>> {
        match &node.kind {
            NodeKind::Func(func_id) => Some(library.by_id(*func_id)?.into()),
            NodeKind::Graph(link) => Some(self.resolve_graph(*link, library)?.ports()),
            NodeKind::Special(special) => Some(special.func().into()),
            NodeKind::GraphInput | NodeKind::GraphOutput => None,
        }
    }

    /// The declared type of input `port`, or `None` when it can't be resolved —
    /// a boundary node (its arity mirrors the enclosing interface, with no
    /// per-port type here) or a missing func/graph. The caller treats `None` as
    /// the polymorphic `Any`.
    pub(crate) fn input_type(&self, library: &Library, port: InputPort) -> Option<DataType> {
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
        let node = self.find(port.node_id, NodeSearch::TopLevel)?;
        self.node_ports(node, library)?.inputs.get(port.port_idx)
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

    /// The declared [`FuncOutput`] of output `port` — its name + [`OutputType`],
    /// or `None` for a boundary / unresolved node. The output-side mirror of
    /// [`Self::input_spec`].
    fn output_spec<'a>(&'a self, library: &'a Library, port: OutputPort) -> Option<&'a FuncOutput> {
        let node = self.find(port.node_id, NodeSearch::TopLevel)?;
        self.node_ports(node, library)?.outputs.get(port.port_idx)
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
