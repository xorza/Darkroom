//! [`Subgraph`]: a graph with an interface, held by a document and placed as instances.

use serde::{Deserialize, Serialize};

use crate::graph::Graph;
use crate::graph::identity::NodeId;
use crate::graph::interface::Interface;
use crate::graph::node::{Boundary, Node};

/// A graph with an interface, held by a document; a node of it is an *instance*. One kind for
/// every use: a subgraph used once behaves as a private one, as Blender's node groups, shared
/// data-blocks with a user count
/// ([node groups](https://docs.blender.org/manual/en/latest/interface/controls/nodes/groups.html)).
///
/// Its graph holds exactly one `Inputs` and one `Outputs` node, which show the interface from
/// the inside; the host's edit gate keeps them there, as it keeps every other rule of a graph.
/// An instance, an `Inputs` and an `Outputs` node each resolve to the ports the interface gives
/// them ([`NodeDecl`](crate::NodeDecl)), with no body: the compile removes all three.
///
/// Plain data. An instance's wiring names the interface's ports by id, so a host that sets
/// another interface removes, in the same edit, each binding on an instance or a boundary node
/// that names a port the new interface lacks or no longer admits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subgraph {
    pub name: String,
    pub description: Option<String>,
    pub interface: Interface,
    pub graph: Graph,
}

impl Subgraph {
    /// A subgraph named `name` with `interface`, its graph holding the two boundary nodes alone.
    pub fn new(name: impl Into<String>, interface: Interface) -> Self {
        let mut graph = Graph::default();
        graph.add(Node::from(Boundary::Inputs));
        graph.add(Node::from(Boundary::Outputs));
        Self {
            name: name.into(),
            description: None,
            interface,
            graph,
        }
    }

    /// The node of this subgraph's graph on `boundary`'s side.
    ///
    /// # Panics
    /// If the graph holds no such node, which a subgraph a host keeps always does.
    pub fn boundary_node(&self, boundary: Boundary) -> NodeId {
        self.graph
            .iter()
            .find(|node| node.boundary() == Some(boundary))
            .expect("a subgraph's graph holds each boundary node")
            .node_id
    }
}
