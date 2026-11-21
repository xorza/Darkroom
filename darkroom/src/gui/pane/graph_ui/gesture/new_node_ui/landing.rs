//! [`Landing`] and [`DroppedEnd`]: which port of a palette pick's new node a dropped wire ends
//! on, and what a port must be to take it.

use scenarium::{
    Binding, InputDecl, InputPort, Library, NodeId, OutputDecl, OutputPort, PortId, Ports,
};

use crate::core::edit::graph_intent::GraphIntent;
use crate::gui::graph_ctx::GraphCtx;
use crate::gui::port_ref::{PortKind, PortRef};

/// What a row lands a dropped wire on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Landing {
    /// The port of the new node the wire ends on, the side opposite the wire's source.
    pub(super) port: PortId,
}

/// The end a wire dropped on the canvas grew out of, with the declaration the edit gate reads of
/// it: a port of a new node takes the wire when [`Library::admit_wire`] admits the two.
#[derive(Clone, Copy, Debug)]
pub(super) enum DroppedEnd<'a> {
    /// Grown out of an output, which a new node's input reads.
    Output {
        source: OutputPort,
        decl: &'a OutputDecl,
        /// Whether the output holds only a constant, which a const-only input admits.
        is_const: bool,
    },
    /// Grown out of an input, which a new node's output feeds.
    Input {
        target: InputPort,
        decl: &'a InputDecl,
    },
}

impl<'a> DroppedEnd<'a> {
    /// The end `source` names in `graph_ctx`'s graph, or `None` once its node is gone: an undo
    /// between the drop and the pick can take it.
    pub(super) fn of(graph_ctx: GraphCtx<'a>, source: PortRef) -> Option<Self> {
        let decl = graph_ctx
            .scope()
            .decl(graph_ctx.body().find(source.node_id)?);
        Some(match source.kind {
            PortKind::Output => Self::Output {
                source: source.output_port().expect("an output's port"),
                decl: &decl.ports().output(source.port)?.port,
                is_const: decl.output_is_const(source.port),
            },
            PortKind::Input => Self::Input {
                target: source.input_port().expect("an input's port"),
                decl: &decl.ports().input(source.port)?.port,
            },
        })
    }

    /// Hand `each` every port of a new node declaring `ports` that takes the wire, in declared
    /// order. A new node's output holds more than a constant: an `Inputs` node, the one whose
    /// outputs mirror constants, lists no ports for a dropped wire.
    pub(super) fn each_landing(
        self,
        library: &Library,
        ports: &Ports,
        mut each: impl FnMut(Landing),
    ) {
        match self {
            Self::Output { decl, is_const, .. } => {
                for input in &ports.inputs {
                    if library.admit_wire(input, decl, is_const).is_ok() {
                        each(Landing { port: input.id });
                    }
                }
            }
            Self::Input { decl, .. } => {
                for output in &ports.outputs {
                    if library.admit_wire(decl, output, false).is_ok() {
                        each(Landing { port: output.id });
                    }
                }
            }
        }
    }

    /// The name of `landing`'s port among `ports`, on the side the wire lands on.
    pub(super) fn landing_name(self, ports: &Ports, landing: Landing) -> &str {
        let name = match self {
            Self::Output { .. } => ports.input(landing.port).map(|input| input.name.as_str()),
            Self::Input { .. } => ports
                .output(landing.port)
                .map(|output| output.name.as_str()),
        };
        name.expect("a landing is a port its entry declares")
    }

    /// The edit that lays the wire onto `landing` of the new node `node_id`.
    pub(super) fn wire(self, node_id: NodeId, landing: Landing) -> GraphIntent {
        match self {
            Self::Output { source, .. } => {
                GraphIntent::set_input(InputPort::new(node_id, landing.port), Binding::Wire(source))
            }
            Self::Input { target, .. } => GraphIntent::set_input(
                target,
                Binding::Wire(OutputPort::new(node_id, landing.port)),
            ),
        }
    }
}
