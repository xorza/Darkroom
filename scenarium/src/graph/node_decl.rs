//! [`NodeDecl`]: what a node resolves to, a func or a subgraph's face.

use crate::graph::func::{Func, FuncEvent};
use crate::graph::identity::PortId;
use crate::graph::node::Boundary;
use crate::graph::ports::EventDecl;
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

    /// The events the node fires: a func's, an instance's interface events, and the `Inputs`
    /// node's interface trigger, which it fires inside.
    pub fn events(self) -> NodeEvents<'a> {
        match self {
            Self::Func(func) => NodeEvents::Func(func.events()),
            Self::Instance(subgraph) => NodeEvents::Declared(subgraph.interface.events()),
            Self::Boundary {
                side: Boundary::Inputs,
                owner,
            } => NodeEvents::Declared(owner.interface.inputs_node_events()),
            Self::Boundary {
                side: Boundary::Outputs,
                ..
            } => NodeEvents::Declared(&[]),
        }
    }

    /// The event under `id`, if the node fires one.
    pub fn event(self, id: PortId) -> Option<EventRef<'a>> {
        self.events().get(id)
    }

    /// The triggers the node hears: the `Outputs` node's interface events, which it hears
    /// inside. A func and an instance hear on their pin.
    pub fn triggers(self) -> &'a [Numbered<EventDecl>] {
        match self {
            Self::Boundary {
                side: Boundary::Outputs,
                owner,
            } => owner.interface.events(),
            Self::Func(_)
            | Self::Instance(_)
            | Self::Boundary {
                side: Boundary::Inputs,
                ..
            } => &[],
        }
    }

    /// Whether the node hears on its pin: a sink func, which a subscription runs, and an
    /// instance whose interface declares a trigger, which a subscription passes into.
    pub const fn hears_pin(self) -> bool {
        match self {
            Self::Func(func) => func.sink(),
            Self::Instance(subgraph) => subgraph.interface.trigger().is_some(),
            Self::Boundary { .. } => false,
        }
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

/// The events a node fires, as its declaration holds them: a func's beside their lambdas, or an
/// interface's.
#[derive(Clone, Copy, Debug)]
pub enum NodeEvents<'a> {
    Func(&'a [FuncEvent]),
    Declared(&'a [Numbered<EventDecl>]),
}

/// One event a node fires: its id and its name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EventRef<'a> {
    pub id: PortId,
    pub name: &'a str,
}

impl<'a> NodeEvents<'a> {
    pub const fn len(self) -> usize {
        match self {
            Self::Func(events) => events.len(),
            Self::Declared(events) => events.len(),
        }
    }

    pub const fn is_empty(self) -> bool {
        self.len() == 0
    }

    /// In the order the node shows them.
    pub fn iter(self) -> impl ExactSizeIterator<Item = EventRef<'a>> {
        let (funcs, declared) = match self {
            Self::Func(events) => (events, &[][..]),
            Self::Declared(events) => (&[][..], events),
        };
        let funcs = funcs.iter().map(|event| EventRef {
            id: event.id,
            name: &event.name,
        });
        let declared = declared.iter().map(|event| EventRef {
            id: event.id,
            name: &event.name,
        });
        ExactChain(funcs.chain(declared), self.len())
    }

    /// The event under `id`, if the node fires one.
    pub fn get(self, id: PortId) -> Option<EventRef<'a>> {
        self.iter().find(|event| event.id == id)
    }
}

/// A chain whose length is known: one of its halves is empty.
#[derive(Debug)]
struct ExactChain<I>(I, usize);

impl<I: Iterator> Iterator for ExactChain<I> {
    type Item = I::Item;

    fn next(&mut self) -> Option<I::Item> {
        let next = self.0.next();
        if next.is_some() {
            self.1 -= 1;
        }
        next
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.1, Some(self.1))
    }
}

impl<I: Iterator> ExactSizeIterator for ExactChain<I> {}
