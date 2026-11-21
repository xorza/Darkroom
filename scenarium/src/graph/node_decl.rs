//! [`NodeDecl`]: what a node resolves to, a func or a compound's face.

use crate::graph::compound::Compound;
use crate::graph::func::{Func, FuncEvent};
use crate::graph::identity::{PortId, TriggerPort};
use crate::graph::node::Boundary;
use crate::graph::ports::EventDecl;
use crate::graph::ports::{InputDecl, Numbered, OutputDecl, Ports};

/// What a node resolves to through a [`Scope`](crate::Scope): the ports it shows and is wired
/// by, and a body if it runs. A func is a declaration with a body; an instance and a boundary
/// node show the ports their compound's interface gives them and have none, as the compile
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
    /// An instance of the compound.
    Instance(&'a Compound),
    /// The boundary node on `side` of `owner`'s graph.
    Boundary { side: Boundary, owner: &'a Compound },
}

impl<'a> NodeDecl<'a> {
    /// The name the declaration gives a new node.
    pub fn name(self) -> &'a str {
        match self {
            Self::Func(func) => func.name(),
            Self::Instance(compound) => &compound.name,
            Self::Boundary { side, .. } => side.name(),
        }
    }

    pub fn description(self) -> Option<&'a str> {
        match self {
            Self::Func(func) => func.description(),
            Self::Instance(compound) => compound.description.as_deref(),
            Self::Boundary { .. } => None,
        }
    }

    pub const fn ports(self) -> &'a Ports {
        match self {
            Self::Func(func) => func.ports(),
            Self::Instance(compound) => compound.interface.ports(),
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
            Self::Instance(compound) => NodeEvents::Declared(compound.interface.events()),
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

    /// The triggers the node shows: its own, for a sink func, which a subscription runs, and for
    /// an instance whose interface declares a trigger, which a subscription passes into; one per
    /// interface event for the `Outputs` node, which passes each out.
    pub fn triggers(self) -> NodeTriggers<'a> {
        match self {
            Self::Func(func) if func.sink() => NodeTriggers::Own,
            Self::Instance(compound) if compound.interface.trigger().is_some() => NodeTriggers::Own,
            Self::Boundary {
                side: Boundary::Outputs,
                owner,
            } => NodeTriggers::PerEvent(owner.interface.events()),
            Self::Func(_)
            | Self::Instance(_)
            | Self::Boundary {
                side: Boundary::Inputs,
                ..
            } => NodeTriggers::None,
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
    /// input inside the compound may take a wire from it.
    pub fn output_is_const(self, port: PortId) -> bool {
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

/// The triggers a node shows, where a subscription lands: no node has both its own and others.
#[derive(Clone, Copy, Debug)]
pub enum NodeTriggers<'a> {
    None,
    /// Its own, under [`TriggerPort::NODE`].
    Own,
    /// The `Outputs` node's, one per interface event, each under the event's id and named as it.
    PerEvent(&'a [Numbered<EventDecl>]),
}

impl NodeTriggers<'_> {
    /// Whether the node shows a trigger under `id`.
    pub fn contains(self, id: PortId) -> bool {
        match self {
            Self::None => false,
            Self::Own => id == TriggerPort::NODE,
            Self::PerEvent(events) => events.iter().any(|event| event.id == id),
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
        (0..self.len()).map(move |idx| match self {
            Self::Func(events) => EventRef {
                id: events[idx].id,
                name: &events[idx].name,
            },
            Self::Declared(events) => EventRef {
                id: events[idx].id,
                name: &events[idx].name,
            },
        })
    }

    /// The event under `id`, if the node fires one.
    pub fn get(self, id: PortId) -> Option<EventRef<'a>> {
        self.iter().find(|event| event.id == id)
    }
}
