//! [`GhostPort`], [`GhostEvent`] and [`GhostTrigger`]: the ports a node shows that no
//! declaration gives it.

use scenarium::{DataType, EventPort, GhostBinding, TriggerPort};

use crate::gui::port_ref::PortRef;

/// One port a node shows that its declaration does not: each port of a ghost node, and on a
/// node each port its ghost wiring names that it no longer declares. A row with a hollow dot,
/// which nothing wires to and which goes with what it holds.
#[derive(Clone, Copy, Debug)]
pub(crate) struct GhostPort<'a> {
    pub(crate) port: PortRef,
    /// The name the port had, when a save kept it: else the row reads by the port's number.
    pub(crate) name: Option<&'a str>,
    /// The type it had, likewise: else its dot wears no type's hue.
    pub(crate) data_type: Option<DataType>,
    /// What an input held, a ghost wire or a ghost constant; `None` for an output, and for an
    /// input of a ghost node that held nothing.
    pub(crate) held: Option<&'a GhostBinding>,
}

/// One event a node shows that its declaration does not fire; [`GhostPort`] for an event.
#[derive(Clone, Copy, Debug)]
pub(crate) struct GhostEvent<'a> {
    pub(crate) event: EventPort,
    pub(crate) name: Option<&'a str>,
}

/// One trigger a node shows that its declaration does not; [`GhostEvent`] for a trigger.
#[derive(Clone, Copy, Debug)]
pub(crate) struct GhostTrigger<'a> {
    pub(crate) trigger: TriggerPort,
    pub(crate) name: Option<&'a str>,
}
