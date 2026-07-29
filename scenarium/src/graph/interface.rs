//! The interface a node presents: input/output/event arity, declared types,
//! and the declaration flags behind them — whatever declares it. A [`Func`]
//! and a [`GraphDef`](crate::graph::definition::GraphDef) answer the same
//! questions here, so a caller reading a node's ports never learns which kind
//! it instantiates. Shared by compile-time validation and the editor.

use crate::graph::definition::GraphEvent;
use crate::graph::node::definition::{Func, FuncBehavior, FuncEvent, FuncInput, FuncOutput};
use crate::graph::{Binding, InputPort, NodeId};

/// The ports a node declares, borrowed from whatever declares them: a [`Func`]
/// in the library, a [`GraphDef`](crate::graph::definition::GraphDef)'s
/// interface, or a special node's hardcoded
/// spec. Resolved once by
/// [`Graph::node_ports`](crate::graph::Graph::node_ports) so no caller
/// repeats the
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
    /// The func this node instantiates, if any — `None` for a composite,
    /// which has no declaration of its own. Read the declaration *flags*
    /// through [`sink`](Self::sink) / [`uncacheable`](Self::uncacheable) /
    /// [`impure`](Self::impure) rather than through here: what each means for
    /// a composite is this type's answer to give, not each caller's.
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
    /// Whether this node performs sink work — no outputs anything downstream
    /// consumes, so a run has to reach it explicitly.
    ///
    /// A composite has no declaration of its own to read, and whether its
    /// *interior* holds sink work is a question only a compiled program can
    /// answer ([`CompiledGraph::is_sink`](crate::CompiledGraph::is_sink)).
    /// "Exposes no outputs" stands in until one exists — which is what an
    /// editor needs before the first compile, and what a composite that
    /// relays nothing outward means anyway.
    pub fn sink(self) -> bool {
        self.func
            .map_or_else(|| self.outputs.is_empty(), |func| func.sink)
    }

    /// Whether the node manages its own output caching, so a storage policy
    /// set on it would mean nothing. A composite is never uncacheable itself:
    /// its storage is its interior's business.
    pub fn uncacheable(self) -> bool {
        self.func.is_some_and(|func| func.uncacheable)
    }

    /// Whether this node recomputes every run — it has no content digest, so
    /// no cache mode is honored on it.
    ///
    /// `false` for a composite: aggregate purity is a property of the
    /// interior, so it too waits on a compiled program
    /// ([`CompiledGraph::is_impure`](crate::CompiledGraph::is_impure))
    /// rather than being guessed here.
    pub fn impure(self) -> bool {
        self.func
            .is_some_and(|func| func.behavior == FuncBehavior::Impure)
    }

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
