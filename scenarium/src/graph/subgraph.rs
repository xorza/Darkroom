//! [`Subgraph`]: a graph with an interface, held by a document and placed as instances.
//!
//! # Decisions
//!
//! - **One kind of subgraph.** Every subgraph is a definition the document holds and every node
//!   of it an instance; one used once behaves as a private one. As Blender's node groups, shared
//!   data-blocks with a user count ([node
//!   groups](https://docs.blender.org/manual/en/latest/interface/controls/nodes/groups.html)),
//!   [Grasshopper's clusters](https://www.grasshopper3d.com/page/clusters) and [Node-RED's
//!   subflows](https://nodered.org/docs/user-guide/editor/workspace/subflows). Refused: a private
//!   group beside a shared function, as Unreal's collapsed graph beside its function ([collapsing
//!   graphs](https://dev.epicgames.com/documentation/en-us/unreal-engine/collapsing-graphs-in-unreal-engine))
//!   and Nuke's Group beside its gizmo
//!   ([gizmos](https://learn.foundry.com/nuke/content/comp_environment/configuring_nuke/creating_sourcing_gizmos.html)):
//!   two routes to one result, a graph stored by value in one and by id in the other.
//! - **One `Inputs` node and one `Outputs` node, each a node told by its kind.** Each port is a
//!   row on it, and placement, selection and wiring treat it as any node, as Blender's group input
//!   and output, Unreal's function entry and result, and
//!   [ComfyUI](https://docs.comfy.org/interface/features/subgraph). Refused: a node per port, as
//!   Nuke's [Input](https://learn.foundry.com/nuke/content/reference_guide/other_nodes/input.html),
//!   whose order needs a hidden number that scripts repair ([pipe
//!   numbers](https://www.nukepedia.com/knowledge/general-tutorials/written-tutorials/working-with-pipe-numbers/));
//!   two reserved node ids, as ComfyUI's `-10` and `-20` ([node
//!   ids](https://docs.comfy.org/custom-nodes/js/node-id-migration)), which rule out several
//!   `Inputs` nodes later.

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
