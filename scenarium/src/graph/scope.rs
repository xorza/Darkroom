//! [`Scope`]: what the nodes of one graph resolve against.

use crate::graph::error::Unresolved;
use crate::graph::graph_set::GraphSet;
use crate::graph::node::{BodySource, Node, NodeKind};
use crate::graph::node_decl::NodeDecl;
use crate::graph::subgraph::Subgraph;
use crate::library::Library;

/// What the nodes of one graph resolve against: the library's funcs, the set's subgraphs, and,
/// for a subgraph's own graph, the interface its boundary nodes show. Made by
/// [`GraphSet::scope`].
///
/// The library stays the host's registry of funcs, fixed for a process; the subgraphs are the
/// document's. Refused: writing a func for each subgraph into the library, which the load
/// contract takes as fixed.
#[derive(Clone, Copy, Debug)]
pub struct Scope<'a> {
    library: &'a Library,
    graphs: &'a GraphSet,
    /// The subgraph whose graph this is: `None` for the main graph.
    owner: Option<&'a Subgraph>,
}

impl<'a> Scope<'a> {
    /// The scope of the graph of `owner`, a subgraph `graphs` holds, or of the main graph.
    pub(crate) const fn new(
        library: &'a Library,
        graphs: &'a GraphSet,
        owner: Option<&'a Subgraph>,
    ) -> Self {
        Self {
            library,
            graphs,
            owner,
        }
    }

    pub const fn library(&self) -> &'a Library {
        self.library
    }

    /// The subgraph whose graph this is, or `None` for the main graph.
    pub const fn owner(&self) -> Option<&'a Subgraph> {
        self.owner
    }

    /// What `node` declares, or why it declares nothing here: no func or subgraph under its id,
    /// or a boundary node outside a subgraph. A declaration whose ports changed since the node
    /// was authored still resolves: the node's wiring names ports by id, and the load admits
    /// each binding against the ports as they are.
    pub fn resolve(&self, node: &Node) -> Result<NodeDecl<'a>, Unresolved> {
        match node.kind {
            NodeKind::Body {
                source: BodySource::Func(func_id),
                ..
            } => self
                .library
                .by_id(func_id)
                .map(NodeDecl::Func)
                .ok_or(Unresolved::MissingFunc),
            NodeKind::Body {
                source: BodySource::Special(special),
                ..
            } => Ok(NodeDecl::Func(special.func())),
            NodeKind::Instance { subgraph, .. } => self
                .graphs
                .subgraph(subgraph)
                .map(NodeDecl::Instance)
                .ok_or(Unresolved::MissingSubgraph),
            NodeKind::Boundary(side) => self
                .owner
                .map(|owner| NodeDecl::Boundary { side, owner })
                .ok_or(Unresolved::NoInterface),
        }
    }
}
