//! [`NodeDecl`]: what a node resolves to, a func or a compound's face.

use crate::decl::func::Func;
use crate::decl::ports::{EventDecl, InputDecl, Numbered, OutputDecl, Ports};
use crate::decl::saved_decl::{SavedDecl, SavedEvent, SavedInput, SavedPort};
use crate::graph::compound::Compound;
use crate::graph::node::Boundary;
use crate::identity::{PortId, TriggerPort};

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
    pub fn events(self) -> &'a [Numbered<EventDecl>] {
        match self {
            Self::Func(func) => func.events(),
            Self::Instance(compound) => compound.interface.events(),
            Self::Boundary {
                side: Boundary::Inputs,
                owner,
            } => owner.interface.inputs_node_events(),
            Self::Boundary {
                side: Boundary::Outputs,
                ..
            } => &[],
        }
    }

    /// The event under `id`, if the node fires one.
    pub fn event(self, id: PortId) -> Option<&'a Numbered<EventDecl>> {
        self.events().iter().find(|event| event.id == id)
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

impl NodeDecl<'_> {
    /// This declaration as a document remembers it.
    pub fn saved(self) -> SavedDecl {
        SavedDecl {
            name: self.name().to_owned(),
            category: self
                .func()
                .map_or_else(String::new, |func| func.category().to_owned()),
            inputs: self
                .inputs()
                .iter()
                .map(|input| {
                    Numbered::new(
                        input.id,
                        SavedInput {
                            name: input.name.clone(),
                            data_type: input.data_type,
                            default_value: input
                                .default_value
                                .clone()
                                .filter(|_| !input.data_type.is_path()),
                        },
                    )
                })
                .collect(),
            outputs: self
                .outputs()
                .iter()
                .map(|output| {
                    Numbered::new(
                        output.id,
                        SavedPort {
                            name: output.name.clone(),
                            data_type: output.data_type,
                        },
                    )
                })
                .collect(),
            events: self
                .events()
                .iter()
                .map(|event| {
                    Numbered::new(
                        event.id,
                        SavedEvent {
                            name: event.name.clone(),
                        },
                    )
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::data::const_value::ConstValue;
    use crate::data::data_type::DataType;
    use crate::decl::ports::InputDecl;
    use crate::graph::node_decl::NodeDecl;
    use crate::identity::{FuncId, PortId};
    use crate::internals;

    /// A saved declaration keeps an input's default, but not a path input's: that default is the
    /// machine's, and the file would carry its folders.
    #[test]
    fn a_saved_declaration_keeps_no_path_default() {
        let func = internals::stub_func(FuncId::unique(), "model")
            .input(1, InputDecl::optional("gain", DataType::Float).default(0.5))
            .input(
                2,
                InputDecl::optional("model", DataType::Dir)
                    .default(ConstValue::FsPath("/home/someone/models".to_owned())),
            )
            .build();
        let saved = NodeDecl::Func(&func).saved();
        let default = |id| saved.input(PortId::new(id)).unwrap().default_value.clone();
        assert_eq!(default(1), Some(ConstValue::Float(0.5)));
        assert_eq!(default(2), None);
    }
}
