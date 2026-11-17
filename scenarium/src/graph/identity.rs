//! How an authored graph names things: the uuid identities ([`NodeId`],
//! [`FuncId`], [`CompoundId`]), a port's number ([`PortId`]), the graph a node lies in ([`GraphId`],
//! [`NodeAddr`]), the addresses that pair a node with a port's id ([`InputPort`],
//! [`OutputPort`], [`EventPort`]), and the crate-internal [`FuncRef`] that pairs a func's id
//! with the signature of its ports for the compiled program.
//!
//! Gathered here so "what can this model address?" is one file. They carry no authoring state,
//! so the execution side, and [`error`](crate::graph::error)'s reports, name nodes and ports
//! without reaching into the model behind them.
//!
//! The three port addresses are the same shape on purpose and are written out rather than
//! generated, so `pub struct InputPort` is greppable. They carry **one derive list, in one
//! order**, so none is missing a bound its siblings have.

use std::fmt;
use std::num::NonZeroU32;

use ::serde::{Deserialize, Serialize};
use common::Id;

use crate::graph::compound::Compound;
use crate::graph::func::Func;
use crate::graph::func::signature::FuncSignature;
use crate::graph::node::Node;

/// A node's id, unique inside its graph: an authored node is addressed by its graph and its id
/// ([`NodeAddr`]). As ComfyUI, where a node id is "local to its immediate graph level"
/// ([subgraphs](https://docs.comfy.org/custom-nodes/js/subgraphs)), so a copy of a compound
/// keeps its node ids and stays equal to its source, as Blender's plain tree copy keeps its node
/// identifiers
/// ([persistent node identifiers](https://projects.blender.org/archive/blender-archive/commit/90ea1b76434fe175)).
/// Refused: ids unique across the document, a rule a file can break with no sound repair, and one
/// that makes every copy mint ids and differ from its source for good.
pub type NodeId = Id<Node>;
/// A declaration's id in the library, not anything in a document.
pub type FuncId = Id<Func>;
/// A compound's id among the compounds a document holds.
pub type CompoundId = Id<Compound>;
/// A port's id, unique among the ports of its side of a declaration: the number a func gives the
/// port, or the random one an interface mints, so a reorder and a rename keep it. Written as the
/// bare number, and never 0, so no port is one "not numbered yet".
///
/// 32 bits, the width a func's number has: a number once shipped is never changed or given to
/// another port, as a protobuf field's, "a number between 1 and 536,870,911"
/// ([field numbers](https://protobuf.dev/programming-guides/proto3/#assigning)). Cost: two copies
/// of a compound changed apart, each gaining `k` ports on a side, share an id with probability
/// about `k² / 2³²`, 6 in 10⁸ for 16 each, where a UUID made it never. Refused: 64 bits, 4
/// bytes more on every port address to improve odds already past caring; 16 bits, where a config
/// struct of 100 fields numbered by a hash collides in 1 build of 13 (`100² / 2 / 2¹⁶`); a
/// counter in each interface, as Blender's `next_uid`
/// ([`DNA_node_tree_interface_types.h`](https://github.com/blender/blender/blob/main/source/blender/makesdna/DNA_node_tree_interface_types.h)),
/// which gives both copies the same next number, so an update binds a wire to another port.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(transparent)]
pub struct PortId(NonZeroU32);

impl PortId {
    /// The lowest id: where a range over one node's ports starts.
    pub const MIN: Self = Self(NonZeroU32::MIN);

    /// The id of the port a declaration numbers `number`: what
    /// [`FuncBuilder::input`](crate::FuncBuilder::input) and its siblings give the port. A
    /// shipped number is never changed or given to another port, as a saved node's wiring names
    /// the port by it.
    ///
    /// # Panics
    /// If `number` is 0.
    pub const fn new(number: u32) -> Self {
        match NonZeroU32::new(number) {
            Some(number) => Self(number),
            None => panic!("a port's number is not 0"),
        }
    }

    pub const fn get(self) -> u32 {
        self.0.get()
    }

    /// A random id that `taken` does not hold: what an interface gives a new port, so two copies
    /// of a definition changed apart give one id to two ports only by the odds above.
    pub fn mint(taken: impl Fn(Self) -> bool) -> Self {
        // The low 32 bits of a version 4 UUID are random.
        Self::mint_from(Id::<Self>::unique().as_u128() as u32, taken)
    }

    /// The first id from `seed` upward, past `u32::MAX` to 1, that `taken` does not hold.
    fn mint_from(seed: u32, taken: impl Fn(Self) -> bool) -> Self {
        (0..=u32::MAX)
            .filter_map(|step| NonZeroU32::new(seed.wrapping_add(step)))
            .map(Self)
            .find(|id| !taken(*id))
            .expect("a side holds fewer than 2³² − 1 ports")
    }
}

/// Which graph of a document: the main graph, or a compound's.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum GraphId {
    Main,
    Compound(CompoundId),
}

/// An authored node's address in a document: its graph and its id there.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeAddr {
    pub graph: GraphId,
    pub node_id: NodeId,
}

impl NodeAddr {
    pub const fn new(graph: GraphId, node_id: NodeId) -> Self {
        Self { graph, node_id }
    }
}

impl fmt::Display for PortId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// The implementation a compiled node runs: its func's id and the signature of its ports, which
/// a node's state and cached values are owned by. Internal to the crate: a node names its func by
/// its [`FuncId`] alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct FuncRef {
    id: FuncId,
    signature: FuncSignature,
}

impl FuncRef {
    pub(crate) const fn new(id: FuncId, signature: FuncSignature) -> Self {
        Self { id, signature }
    }

    pub(crate) const fn id(self) -> FuncId {
        self.id
    }

    pub(crate) const fn signature(self) -> FuncSignature {
        self.signature
    }
}

/// Address of a producer node's output port, the source side of a `Binding::Wire`, by the id
/// its declaration gives the port.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OutputPort {
    pub node_id: NodeId,
    pub port: PortId,
}

impl OutputPort {
    pub const fn new(node_id: NodeId, port: PortId) -> Self {
        Self { node_id, port }
    }
}

/// Address of an emitter node's event, by the id its declaration gives it.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EventPort {
    pub node_id: NodeId,
    pub event: PortId,
}

impl EventPort {
    pub const fn new(node_id: NodeId, event: PortId) -> Self {
        Self { node_id, event }
    }
}

/// Address of a trigger, where a subscription lands: a node's own trigger under [`Self::NODE`],
/// or the `Outputs` node's trigger under the id of the interface event it passes out.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TriggerPort {
    pub node_id: NodeId,
    pub trigger: PortId,
}

impl TriggerPort {
    /// The id of a node's own trigger: a sink's, or an instance's whose interface declares one. No
    /// node has both its own trigger and others, so it shares no id.
    pub const NODE: PortId = PortId::new(1);

    pub const fn new(node_id: NodeId, trigger: PortId) -> Self {
        Self { node_id, trigger }
    }
}

impl From<NodeId> for TriggerPort {
    /// The node's own trigger.
    fn from(node_id: NodeId) -> Self {
        Self::new(node_id, Self::NODE)
    }
}

/// Address of a consumer node's input port; keys `Graph.bindings`. Distinct from `OutputPort`
/// so source and sink can't be confused.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InputPort {
    pub node_id: NodeId,
    pub port: PortId,
}

impl InputPort {
    pub const fn new(node_id: NodeId, port: PortId) -> Self {
        Self { node_id, port }
    }
}

#[cfg(test)]
mod tests {
    use common::SerdeFormat;

    use crate::graph::identity::PortId;

    /// A mint takes its seed when free, the next free number when not, and wraps past
    /// `u32::MAX` over 0, which is no id, to 1.
    #[test]
    fn a_mint_skips_each_taken_id_and_zero() {
        assert_eq!(PortId::mint_from(5, |_| false), PortId::new(5));
        assert_eq!(
            PortId::mint_from(5, |id| matches!(id.get(), 5 | 6)),
            PortId::new(7)
        );
        assert_eq!(PortId::mint_from(0, |_| false), PortId::new(1));
        assert_eq!(
            PortId::mint_from(u32::MAX, |id| id.get() == u32::MAX),
            PortId::new(1)
        );
        let minted = PortId::mint(|id| id.get() % 2 == 0);
        assert_eq!(minted.get() % 2, 1);
    }

    /// A port id is written as its bare number, and a file's 0 is refused where it is read.
    #[test]
    fn a_port_id_is_its_number_in_a_file() {
        let ron = SerdeFormat::Ron.serialize(&PortId::new(3)).unwrap();
        assert_eq!(String::from_utf8(ron).unwrap().trim(), "3");
        assert_eq!(
            SerdeFormat::Ron.deserialize::<PortId>(b"3").unwrap(),
            PortId::new(3)
        );
        assert!(SerdeFormat::Ron.deserialize::<PortId>(b"0").is_err());
    }

    #[test]
    #[should_panic(expected = "a port's number is not 0")]
    fn the_number_zero_is_no_port() {
        let _ = PortId::new(0);
    }
}
