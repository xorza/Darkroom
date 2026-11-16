//! [`NodeDecl`]: what a node resolves to, a func or a subgraph's face.

use crate::graph::func::{Func, FuncEvent};
use crate::graph::identity::PortId;
use crate::graph::node::Boundary;
use crate::graph::ports::{InputDecl, Numbered, OutputDecl, Ports};
use crate::graph::subgraph::Subgraph;

/// What a node resolves to through a [`Scope`](crate::Scope): the ports it shows and is wired
/// by, and a body if it runs. A func is a declaration with a body; an instance and a boundary
/// node show the ports their subgraph's interface gives them and have none, as the compile
/// removes them.
///
/// As MaterialX, where a `nodedef` declares the ports and an implementation, code or a nodegraph,
/// stands behind it ([node definitions](https://kwokcb.github.io/MaterialX_Learn/documents/node_definitions.html)).
/// Refused: a [`Func`] with no body for the nodes the compile removes, which gives a declaration
/// an id, a behavior and a cache mode it has no value for.
#[derive(Clone, Copy, Debug)]
pub enum NodeDecl<'a> {
    /// A library func or a special node's built-in.
    Func(&'a Func),
    /// An instance of the subgraph.
    Instance(&'a Subgraph),
    /// The boundary node on `side` of `owner`'s graph.
    Boundary { side: Boundary, owner: &'a Subgraph },
}

impl<'a> NodeDecl<'a> {
    /// The name the declaration gives a new node.
    pub fn name(self) -> &'a str {
        match self {
            Self::Func(func) => func.name(),
            Self::Instance(subgraph) => &subgraph.name,
            Self::Boundary { side, .. } => side.name(),
        }
    }

    pub fn description(self) -> Option<&'a str> {
        match self {
            Self::Func(func) => func.description(),
            Self::Instance(subgraph) => subgraph.description.as_deref(),
            Self::Boundary { .. } => None,
        }
    }

    pub const fn ports(self) -> &'a Ports {
        match self {
            Self::Func(func) => func.ports(),
            Self::Instance(subgraph) => subgraph.interface.ports(),
            Self::Boundary { side, owner } => owner.interface.boundary_ports(side),
        }
    }

    pub fn inputs(self) -> &'a [Numbered<InputDecl>] {
        &self.ports().inputs
    }

    pub fn outputs(self) -> &'a [Numbered<OutputDecl>] {
        &self.ports().outputs
    }

    /// The events the node fires: a func's, as an interface has no event ports.
    pub fn events(self) -> &'a [FuncEvent] {
        self.func().map_or(&[], Func::events)
    }

    /// The event under `id`, if the node fires one.
    pub fn event(self, id: PortId) -> Option<&'a FuncEvent> {
        self.func()?.event(id)
    }

    /// The func, for a node with a body.
    pub const fn func(self) -> Option<&'a Func> {
        match self {
            Self::Func(func) => Some(func),
            Self::Instance(_) | Self::Boundary { .. } => None,
        }
    }

    /// Whether the output under `port` holds only a constant: the `Inputs` node's mirror of a
    /// const-only interface input, which an instance binds to a constant alone. So a const-only
    /// input inside the subgraph may take a wire from it.
    pub(crate) fn output_is_const(self, port: PortId) -> bool {
        match self {
            Self::Boundary {
                side: Boundary::Inputs,
                owner,
            } => owner
                .interface
                .ports()
                .input(port)
                .is_some_and(|input| input.const_only),
            Self::Func(_) | Self::Instance(_) | Self::Boundary { .. } => false,
        }
    }
}
