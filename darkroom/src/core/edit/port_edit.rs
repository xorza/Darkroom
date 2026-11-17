//! [`PortEdit`]: one change of a compound's interface, as the inspector and the boundary nodes
//! raise it.
//!
//! # Decisions
//!
//! - **A port's type is inferred when the port is made, then held as its declaration**: the first
//!   wire names the type, the name and the default, a collapse reads them off each crossing wire,
//!   and the inspector changes one on request. As Blender's hollow socket. Refused: a type derived
//!   from the inner wiring on every edit, where removing the last inner wire breaks wires on
//!   instances in other graphs; a type picked by hand first, as Unreal's Function Input ([material
//!   function
//!   expressions](https://dev.epicgames.com/documentation/unreal-engine/material-function-expressions-in-unreal-engine)),
//!   two picks where a drag is one.
//! - **A wire from `Inputs` to `Outputs` is an ordinary wire**, as the port it leaves has a type
//!   already. Refused: a blank last row on each boundary node as the drop target, a placeholder
//!   that could be wired to another and name no type.

use scenarium::{
    ConstValue, DataType, EventDecl, InputDecl, Interface, Numbered, OutputDecl, PortId,
    ValueDomain,
};

/// Which list of an interface a port is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InterfaceSide {
    Inputs,
    Outputs,
    /// What an instance fires.
    Events,
}

/// Which way a [`PortEdit::Move`] moves a port in its list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Shift {
    Earlier,
    Later,
}

/// One change of an interface's ports. [`Self::apply`] gives the interface it leaves, which a
/// [`CompoundIntent::SetInterface`](crate::core::edit::compound_intent::CompoundIntent) lands
/// with each binding the new one does not hold dropped.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum PortEdit {
    Rename {
        side: InterfaceSide,
        port: PortId,
        name: String,
    },
    /// Retype a port: a default its new type does not take goes with the old type.
    Retype {
        side: InterfaceSide,
        port: PortId,
        data_type: DataType,
    },
    /// Move a port one place in its list; at the end of the list, nothing.
    Move {
        side: InterfaceSide,
        port: PortId,
        shift: Shift,
    },
    Remove {
        side: InterfaceSide,
        port: PortId,
    },
    /// Set an input's default, the constant a new instance's input starts with, or clear it.
    Default {
        port: PortId,
        value: Option<ConstValue>,
    },
    /// Add a data port at the end of its list, under `port`, a fresh id.
    Add {
        side: DataSide,
        port: PortId,
        name: String,
        data_type: DataType,
    },
    /// Add an event at the end of the list, under `port`, a fresh id.
    AddEvent {
        port: PortId,
        name: String,
    },
    /// Give the interface the trigger an instance hears on its pin, or take it away.
    SetTrigger(Option<EventDecl>),
}

/// The data lists of an interface, the sides a typed port is added to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DataSide {
    Inputs,
    Outputs,
}

impl PortEdit {
    /// The interface this edit leaves of `interface`: the same one for an edit of a port it does
    /// not hold, as a stale edit is, and for a default its port's type does not take.
    pub(crate) fn apply(self, interface: &Interface) -> Interface {
        let mut ports = interface.ports().clone();
        let mut events = interface.event_ports().clone();
        match self {
            Self::Rename { side, port, name } => match side {
                InterfaceSide::Inputs => {
                    if let Some(input) = ports.inputs.iter_mut().find(|input| input.id == port) {
                        input.name = name;
                    }
                }
                InterfaceSide::Outputs => {
                    if let Some(output) = ports.outputs.iter_mut().find(|output| output.id == port)
                    {
                        output.name = name;
                    }
                }
                InterfaceSide::Events => {
                    if let Some(event) = events.events.iter_mut().find(|event| event.id == port) {
                        event.name = name;
                    }
                }
            },
            Self::Retype {
                side,
                port,
                data_type,
            } => match side {
                InterfaceSide::Inputs => {
                    if let Some(input) = ports.inputs.iter_mut().find(|input| input.id == port) {
                        input.data_type = data_type;
                        input.domain = ValueDomain::Any;
                    }
                    // A default the new type does not take goes with the old type.
                    if Interface::checked(ports.clone()).is_err()
                        && let Some(input) = ports.inputs.iter_mut().find(|input| input.id == port)
                    {
                        input.default_value = None;
                    }
                }
                InterfaceSide::Outputs => {
                    if let Some(output) = ports.outputs.iter_mut().find(|output| output.id == port)
                    {
                        output.data_type = data_type;
                    }
                }
                // An event carries no type.
                InterfaceSide::Events => {}
            },
            Self::Move { side, port, shift } => match side {
                InterfaceSide::Inputs => {
                    shift_port(&mut ports.inputs, |input| input.id == port, shift);
                }
                InterfaceSide::Outputs => {
                    shift_port(&mut ports.outputs, |output| output.id == port, shift);
                }
                InterfaceSide::Events => {
                    shift_port(&mut events.events, |event| event.id == port, shift);
                }
            },
            Self::Remove { side, port } => match side {
                InterfaceSide::Inputs => ports.inputs.retain(|input| input.id != port),
                InterfaceSide::Outputs => ports.outputs.retain(|output| output.id != port),
                InterfaceSide::Events => events.events.retain(|event| event.id != port),
            },
            Self::Default { port, value } => {
                if let Some(input) = ports.inputs.iter_mut().find(|input| input.id == port) {
                    input.default_value = value;
                }
            }
            Self::Add {
                side,
                port,
                name,
                data_type,
            } => match side {
                DataSide::Inputs => ports
                    .inputs
                    .push(Numbered::new(port, InputDecl::optional(name, data_type))),
                DataSide::Outputs => ports
                    .outputs
                    .push(Numbered::new(port, OutputDecl::new(name, data_type))),
            },
            Self::AddEvent { port, name } => {
                events
                    .events
                    .push(Numbered::new(port, EventDecl::new(name)));
            }
            Self::SetTrigger(trigger) => events.trigger = trigger,
        }
        // The events go with the data ports, so an edit of one list keeps the others.
        Interface::checked(ports)
            .and_then(|edited| edited.with_event_ports(events))
            .unwrap_or_else(|_| interface.clone())
    }
}

/// Move the item `is` names one place in `items` by `shift`; nothing at the end of the list or
/// for an item `items` lacks.
fn shift_port<T>(items: &mut [T], is: impl Fn(&T) -> bool, shift: Shift) {
    let Some(idx) = items.iter().position(is) else {
        return;
    };
    let to = match shift {
        Shift::Earlier => idx.checked_sub(1),
        Shift::Later => (idx + 1 < items.len()).then_some(idx + 1),
    };
    if let Some(to) = to {
        items.swap(idx, to);
    }
}

#[cfg(test)]
mod tests {
    use scenarium::{
        ConstValue, DataType, InputDecl, Interface, Numbered, OutputDecl, PortId, Ports,
    };

    use crate::core::edit::port_edit::{DataSide, InterfaceSide, PortEdit, Shift};

    fn interface() -> Interface {
        Interface::new(Ports {
            inputs: ["a", "b", "c"]
                .into_iter()
                .zip(1..)
                .map(|(name, id)| {
                    Numbered::new(
                        PortId::new(id),
                        InputDecl::optional(name, DataType::Int)
                            .default(ConstValue::Int(i64::from(id))),
                    )
                })
                .collect(),
            outputs: vec![Numbered::new(
                PortId::new(9),
                OutputDecl::new("out", DataType::Float),
            )],
        })
    }

    fn names(interface: &Interface) -> Vec<&str> {
        interface
            .inputs()
            .iter()
            .map(|input| input.name.as_str())
            .collect()
    }

    /// Each edit by exact result: a move swaps one place and stops at the ends, a rename and a
    /// removal touch the one port, a retype drops a default its type refuses and keeps one it
    /// takes, a default comes and goes, an add goes last, and an edit of a port the interface
    /// lacks changes nothing.
    #[test]
    fn each_edit_leaves_the_interface_it_names() {
        let base = interface();
        let id = PortId::new;
        let inputs = InterfaceSide::Inputs;
        let moved = |port, shift| {
            PortEdit::Move {
                side: inputs,
                port: id(port),
                shift,
            }
            .apply(&base)
        };
        assert_eq!(names(&moved(2, Shift::Earlier)), ["b", "a", "c"]);
        assert_eq!(names(&moved(2, Shift::Later)), ["a", "c", "b"]);
        assert_eq!(
            names(&moved(1, Shift::Earlier)),
            ["a", "b", "c"],
            "the first stays first"
        );
        assert_eq!(
            names(&moved(3, Shift::Later)),
            ["a", "b", "c"],
            "the last stays last"
        );

        let renamed = PortEdit::Rename {
            side: inputs,
            port: id(2),
            name: "gain".to_string(),
        }
        .apply(&base);
        assert_eq!(names(&renamed), ["a", "gain", "c"]);
        let removed = PortEdit::Remove {
            side: inputs,
            port: id(1),
        }
        .apply(&base);
        assert_eq!(names(&removed), ["b", "c"]);

        let retyped = |data_type| {
            PortEdit::Retype {
                side: inputs,
                port: id(1),
                data_type,
            }
            .apply(&base)
            .inputs()[0]
                .port
                .clone()
        };
        let to_any = retyped(DataType::Any);
        assert_eq!(
            (to_any.data_type, to_any.default_value),
            (DataType::Any, Some(ConstValue::Int(1))),
            "an untyped port takes any default"
        );
        let to_float = retyped(DataType::Float);
        assert_eq!(
            (to_float.data_type, to_float.default_value),
            (DataType::Float, None),
            "a declared float takes a float default alone"
        );
        let to_string = retyped(DataType::String);
        assert_eq!(
            (to_string.data_type, to_string.default_value),
            (DataType::String, None)
        );

        let cleared = PortEdit::Default {
            port: id(3),
            value: None,
        }
        .apply(&base);
        assert_eq!(cleared.inputs()[2].default_value, None);
        let set = PortEdit::Default {
            port: id(3),
            value: Some(ConstValue::Int(7)),
        }
        .apply(&base);
        assert_eq!(set.inputs()[2].default_value, Some(ConstValue::Int(7)));

        let added = PortEdit::Add {
            side: DataSide::Outputs,
            port: id(10),
            name: "more".to_string(),
            data_type: DataType::Bool,
        }
        .apply(&base);
        assert_eq!(
            added.outputs().iter().map(|o| o.id).collect::<Vec<_>>(),
            [id(9), id(10)]
        );
        assert_eq!(
            PortEdit::Remove {
                side: inputs,
                port: id(99)
            }
            .apply(&base),
            base
        );
    }

    /// An event is added at the end of the list, renamed, moved and removed as a data port is;
    /// the trigger is set, renamed and taken away; and an edit of the data ports keeps them.
    #[test]
    fn events_and_the_trigger_take_their_edits() {
        use scenarium::EventDecl;

        let id = PortId::new;
        let events = |interface: &Interface| {
            interface
                .events()
                .iter()
                .map(|event| event.name.clone())
                .collect::<Vec<_>>()
        };
        let add = |interface: &Interface, port, name: &str| {
            PortEdit::AddEvent {
                port: id(port),
                name: name.to_string(),
            }
            .apply(interface)
        };
        let with = add(&interface(), 20, "done");
        let with = add(&with, 21, "failed");
        let with = PortEdit::SetTrigger(Some(EventDecl::new("go"))).apply(&with);
        let strings = |names: &[&str]| names.iter().map(ToString::to_string).collect::<Vec<_>>();
        assert_eq!(events(&with), strings(&["done", "failed"]));
        assert_eq!(with.trigger(), Some(&EventDecl::new("go")));
        let moved = PortEdit::Move {
            side: InterfaceSide::Events,
            port: id(21),
            shift: Shift::Earlier,
        }
        .apply(&with);
        assert_eq!(events(&moved), strings(&["failed", "done"]));
        let renamed = PortEdit::Rename {
            side: InterfaceSide::Events,
            port: id(20),
            name: "finished".to_string(),
        }
        .apply(&with);
        assert_eq!(events(&renamed), strings(&["finished", "failed"]));
        let removed = PortEdit::Remove {
            side: InterfaceSide::Events,
            port: id(20),
        }
        .apply(&with);
        assert_eq!(events(&removed), strings(&["failed"]));
        let retriggered = PortEdit::SetTrigger(Some(EventDecl::new("start"))).apply(&with);
        assert_eq!(retriggered.trigger(), Some(&EventDecl::new("start")));
        assert_eq!(events(&retriggered), events(&with), "the events stay");
        let untriggered = PortEdit::SetTrigger(None).apply(&with);
        assert_eq!(untriggered.trigger(), None);
        assert_eq!(events(&untriggered), events(&with), "the events stay");
        let data = PortEdit::Rename {
            side: InterfaceSide::Inputs,
            port: id(1),
            name: "first".to_string(),
        }
        .apply(&with);
        assert_eq!(
            data.event_ports(),
            with.event_ports(),
            "a data edit keeps the events"
        );
        assert_eq!(names(&data)[0], "first");
    }
}
