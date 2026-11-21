//! [`Compound`]: a graph with an interface, held by a document and placed as instances; its
//! [`CompoundDecl`], what its instances show, and the [`CompoundRef`] a set lends.
//!
//! # Decisions
//!
//! - **One kind of compound.** Every compound is a definition the document holds and every node
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
//! - **`Inputs` and `Outputs` nodes, each a node told by its kind.** Each port is a row on it,
//!   and placement, selection and wiring treat it as any node, as Blender's group input and
//!   output, Unreal's function entry and result, and
//!   [ComfyUI](https://docs.comfy.org/interface/features/subgraph). Refused: a node per port, as
//!   Nuke's [Input](https://learn.foundry.com/nuke/content/reference_guide/other_nodes/input.html),
//!   whose order needs a hidden number that scripts repair ([pipe
//!   numbers](https://www.nukepedia.com/knowledge/general-tutorials/written-tutorials/working-with-pipe-numbers/));
//!   two reserved node ids, as ComfyUI's `-10` and `-20` ([node
//!   ids](https://docs.comfy.org/custom-nodes/js/node-id-migration)), which rule out several
//!   `Inputs` nodes.
//! - **One `Outputs` node, and one or more `Inputs` nodes**, each showing the whole interface, so
//!   a long graph reads an input where it is used, as a Blender node group holds several Group
//!   Input nodes ([node groups](https://docs.blender.org/manual/en/latest/interface/controls/nodes/groups.html)).
//!   A wire from any of them reads what the instance holds on that port. Refused: several
//!   `Outputs` nodes, which would give one interface output two producers.
//! - **A compound's declaration stands apart from its body**: [`CompoundDecl`] is what a scope
//!   reads, its name, description and interface, and never its graph, so a graph of a set can
//!   change while a scope over the set lives. As MaterialX, where a `nodedef` declares the ports
//!   and an implementation stands behind it
//!   ([node definitions](https://kwokcb.github.io/MaterialX_Learn/documents/node_definitions.html)).
//!   A file still writes a compound as one value, its declaration's fields beside its graph.

use serde::{Deserialize, Serialize, Serializer};

use crate::graph::Graph;
use crate::graph::interface::Interface;
use crate::graph::node::{Boundary, Node};

/// What a compound's instances and boundary nodes show: its name, what it is for, and its
/// interface. What a [`Scope`](crate::Scope) resolves a node of it to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompoundDecl {
    pub name: String,
    pub description: Option<String>,
    pub interface: Interface,
}

/// A graph with an interface, held by a document; a node of it is an *instance*. One kind for
/// every use: a compound used once behaves as a private one, as Blender's node groups, shared
/// data-blocks with a user count
/// ([node groups](https://docs.blender.org/manual/en/latest/interface/controls/nodes/groups.html)).
///
/// Its graph holds one `Outputs` node and one or more `Inputs` nodes, which show the interface
/// from the inside; the host's edit gate keeps them there, as it keeps every other rule of a
/// graph.
/// An instance, an `Inputs` and an `Outputs` node each resolve to the ports the interface gives
/// them ([`NodeDecl`](crate::NodeDecl)), with no body: the compile removes all three.
///
/// Plain data, the value a document, a clip and a library file move whole. An instance's wiring
/// names the interface's ports by id, so a host that sets another interface removes, in the same
/// edit, each binding on an instance or a boundary node that names a port the new interface lacks
/// or no longer admits.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(from = "CompoundRepr")]
pub struct Compound {
    pub decl: CompoundDecl,
    pub graph: Graph,
}

/// A compound as a [`GraphSet`](crate::GraphSet) holds it: its declaration and its graph,
/// borrowed apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompoundRef<'a> {
    pub decl: &'a CompoundDecl,
    pub graph: &'a Graph,
}

impl Compound {
    /// A compound named `name` with `interface`, its graph holding the two boundary nodes alone.
    pub fn new(name: impl Into<String>, interface: Interface) -> Self {
        let mut graph = Graph::default();
        graph.add(Node::from(Boundary::Inputs));
        graph.add(Node::from(Boundary::Outputs));
        Self {
            decl: CompoundDecl {
                name: name.into(),
                description: None,
                interface,
            },
            graph,
        }
    }

    /// This compound, borrowed as a set lends one.
    pub const fn as_ref(&self) -> CompoundRef<'_> {
        CompoundRef {
            decl: &self.decl,
            graph: &self.graph,
        }
    }
}

impl From<CompoundRef<'_>> for Compound {
    fn from(compound: CompoundRef<'_>) -> Self {
        Self {
            decl: compound.decl.clone(),
            graph: compound.graph.clone(),
        }
    }
}

/// A compound as a file writes it: its declaration's fields, then its graph.
#[derive(Deserialize)]
struct CompoundRepr {
    name: String,
    description: Option<String>,
    interface: Interface,
    graph: Graph,
}

impl From<CompoundRepr> for Compound {
    fn from(repr: CompoundRepr) -> Self {
        Self {
            decl: CompoundDecl {
                name: repr.name,
                description: repr.description,
                interface: repr.interface,
            },
            graph: repr.graph,
        }
    }
}

/// [`CompoundRepr`] borrowed, so a write copies nothing.
#[derive(Serialize)]
struct CompoundReprRef<'a> {
    name: &'a str,
    description: &'a Option<String>,
    interface: &'a Interface,
    graph: &'a Graph,
}

impl Serialize for CompoundRef<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        CompoundReprRef {
            name: &self.decl.name,
            description: &self.decl.description,
            interface: &self.decl.interface,
            graph: self.graph,
        }
        .serialize(serializer)
    }
}

impl Serialize for Compound {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.as_ref().serialize(serializer)
    }
}
