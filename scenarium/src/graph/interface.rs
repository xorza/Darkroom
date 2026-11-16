//! [`Interface`]: the ports a subgraph shows, on its instances and its boundary nodes.
//!
//! # Decisions
//!
//! - **A const-only input takes a wire from a const-only `Inputs` port, and from nothing else.**
//!   An instance holds only a constant there, so the compile lowers the wire to that constant.
//! - **An interface has no event ports**, so a subscription stays inside one graph.

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::graph::node::Boundary;
use crate::graph::ports::{InputDecl, Numbered, OutputDecl, Ports, PortsError};

/// The ports a subgraph shows, each a declaration under its id. An interface port's declaration
/// is that of the port that made it, so a const-only input, a picker and an enum are exposed as
/// they are, and a port's type is held as its declaration, so no edit inside the subgraph
/// retypes the wires on its instances.
///
/// Held to the rules a func's ports pass ([`Ports::check`]), apart from the one that an output is
/// never `Any`: a port made from an `Any` input is an `Any` output of the `Inputs` node.
/// Immutable and built by [`Self::checked`] alone, so what it derives, the ports its two boundary
/// nodes show, is never stale; the file and equality hold the ports alone.
#[derive(Clone, Debug)]
pub struct Interface {
    ports: Ports,
    /// The ports of the `Inputs` node: each interface input as an output of its type.
    inputs_node: Ports,
    /// The ports of the `Outputs` node: each interface output as an optional input, so an
    /// unwired one leaves the instance's output unbound.
    outputs_node: Ports,
}

impl Interface {
    /// # Panics
    /// If `ports` are no interface's: see [`Self::checked`].
    pub fn new(ports: Ports) -> Self {
        match Self::checked(ports) {
            Ok(interface) => interface,
            Err(error) => panic!("{error}"),
        }
    }

    /// The interface of `ports`, or why they are none: what [`Ports::check`] refuses.
    pub fn checked(ports: Ports) -> Result<Self, PortsError> {
        ports.check()?;
        let inputs_node = Ports {
            inputs: Vec::new(),
            outputs: ports
                .inputs
                .iter()
                .map(|input| {
                    Numbered::new(
                        input.id,
                        OutputDecl {
                            description: input.description.clone(),
                            ..OutputDecl::new(input.name.clone(), input.data_type)
                        },
                    )
                })
                .collect(),
        };
        let outputs_node = Ports {
            inputs: ports
                .outputs
                .iter()
                .map(|output| {
                    Numbered::new(
                        output.id,
                        InputDecl {
                            description: output.description.clone(),
                            ..InputDecl::optional(output.name.clone(), output.data_type)
                        },
                    )
                })
                .collect(),
            outputs: Vec::new(),
        };
        Ok(Self {
            ports,
            inputs_node,
            outputs_node,
        })
    }

    /// The ports an instance shows.
    pub const fn ports(&self) -> &Ports {
        &self.ports
    }

    pub fn inputs(&self) -> &[Numbered<InputDecl>] {
        &self.ports.inputs
    }

    pub fn outputs(&self) -> &[Numbered<OutputDecl>] {
        &self.ports.outputs
    }

    /// The ports the boundary node on `side` shows: the interface from inside the subgraph.
    pub const fn boundary_ports(&self, side: Boundary) -> &Ports {
        match side {
            Boundary::Inputs => &self.inputs_node,
            Boundary::Outputs => &self.outputs_node,
        }
    }
}

impl Default for Interface {
    /// The interface of no ports.
    fn default() -> Self {
        Self::new(Ports::default())
    }
}

impl PartialEq for Interface {
    /// What the interface derives is left out, as the ports decide it.
    fn eq(&self, other: &Self) -> bool {
        self.ports == other.ports
    }
}

impl Eq for Interface {}

impl Serialize for Interface {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.ports.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Interface {
    /// Decoded as the ports it holds and checked as [`Self::checked`] checks them, so a file
    /// cannot give two ports one id.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::checked(Ports::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use common::SerdeFormat;

    use crate::data::const_value::ConstValue;
    use crate::data::data_type::file_kind::FileKindId;
    use crate::data::data_type::{DataType, FileMode};
    use crate::graph::identity::PortId;
    use crate::graph::interface::Interface;
    use crate::graph::node::Boundary;
    use crate::graph::ports::{InputDecl, Numbered, OutputDecl, PortList, Ports, PortsError};

    fn input(id: u32) -> Numbered<InputDecl> {
        Numbered::new(
            PortId::new(id),
            InputDecl::optional("in", DataType::Float)
                .default(0.5)
                .description("gain"),
        )
    }

    fn output(id: u32) -> Numbered<OutputDecl> {
        Numbered::new(
            PortId::new(id),
            OutputDecl::new("out", DataType::Int).description("count"),
        )
    }

    fn ports(inputs: Vec<Numbered<InputDecl>>, outputs: Vec<Numbered<OutputDecl>>) -> Ports {
        Ports { inputs, outputs }
    }

    /// The rules a func's ports pass: an id unique on its side, and a default its input takes. An input and an output may share an id, as a func's input 1 and output 1 do.
    #[test]
    fn checked_refuses_what_a_func_refuses() {
        assert_eq!(
            Interface::checked(ports(vec![input(1), input(1)], vec![])).err(),
            Some(PortsError::DuplicateId {
                list: PortList::Inputs,
                idx: 1,
                port_id: PortId::new(1)
            })
        );
        let mut wrong_default = input(1);
        wrong_default.default_value = Some(ConstValue::String("loud".into()));
        assert_eq!(
            Interface::checked(ports(vec![wrong_default], vec![])).err(),
            Some(PortsError::InvalidDefault { idx: 0 })
        );
        let interface = Interface::new(ports(vec![input(1)], vec![output(1)]));
        assert_eq!(interface.inputs(), [input(1)]);
        assert_eq!(interface.outputs(), [output(1)]);
    }

    /// The `Inputs` node shows each interface input as an output of its id, name, type and
    /// description, and the `Outputs` node each interface output as an optional input of the
    /// same, with no default.
    #[test]
    fn each_boundary_node_mirrors_the_ports() {
        let required = Numbered::new(
            PortId::new(2),
            InputDecl::required("path", DataType::Dir).const_only(),
        );
        let interface = Interface::new(ports(vec![input(1), required], vec![output(3)]));
        assert_eq!(
            *interface.boundary_ports(Boundary::Inputs),
            ports(
                vec![],
                vec![
                    Numbered::new(
                        PortId::new(1),
                        OutputDecl::new("in", DataType::Float).description("gain")
                    ),
                    Numbered::new(PortId::new(2), OutputDecl::new("path", DataType::Dir)),
                ],
            )
        );
        assert_eq!(
            *interface.boundary_ports(Boundary::Outputs),
            ports(
                vec![Numbered::new(
                    PortId::new(3),
                    InputDecl::optional("out", DataType::Int).description("count")
                )],
                vec![],
            )
        );
    }

    /// A round trip keeps every port, a file input's kind and mode included, and a document that
    /// repeats a port id is refused at decode rather than loaded with two ports no edit can tell
    /// apart.
    #[test]
    fn an_interface_round_trips_and_a_shared_id_is_refused() {
        let image = Numbered::new(
            PortId::new(2),
            InputDecl::required(
                "image",
                DataType::File {
                    kind: FileKindId::from_u128(9),
                    mode: FileMode::OpenMany,
                },
            ),
        );
        let interface = Interface::new(ports(vec![input(1), image], vec![output(3)]));
        let ron = SerdeFormat::Ron.serialize(&interface).unwrap();
        assert_eq!(
            SerdeFormat::Ron.deserialize::<Interface>(&ron).unwrap(),
            interface
        );

        let ron = String::from_utf8(ron).unwrap();
        assert_eq!(ron.matches("id: 2,").count(), 1, "{ron}");
        let shared = ron.replace("id: 2,", "id: 1,");
        assert!(
            SerdeFormat::Ron
                .deserialize::<Interface>(shared.as_bytes())
                .is_err(),
            "{shared}"
        );
    }
}
