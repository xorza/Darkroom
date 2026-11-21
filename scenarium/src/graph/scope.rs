//! [`Scope`]: what the nodes of one graph resolve against.

use std::collections::BTreeMap;

use crate::graph::compound::CompoundDecl;
use crate::graph::error::Unresolved;
use crate::graph::node::{BodySource, Node, NodeKind};
use crate::graph::node_decl::NodeDecl;
use crate::identity::{CompoundId, GraphId};
use crate::library::Library;

/// What the nodes of one graph resolve against: the library's funcs, the set's compounds, and,
/// for a compound's own graph, the interface its boundary nodes show. Made by
/// [`GraphSet::scope`](crate::GraphSet::scope).
///
/// The library stays the host's registry of funcs, fixed for a process; the compounds are the
/// document's. Refused: writing a func for each compound into the library, which the load
/// contract takes as fixed.
#[derive(Clone, Copy, Debug)]
pub struct Scope<'a> {
    library: &'a Library,
    /// The set's compound declarations, by id.
    decls: &'a BTreeMap<CompoundId, CompoundDecl>,
    /// The compound whose graph this is: `None` for the main graph.
    owner: Option<&'a CompoundDecl>,
}

impl<'a> Scope<'a> {
    /// The scope of `graph`, the main graph or the graph of one of `decls`.
    ///
    /// # Panics
    /// If `graph` names a compound `decls` does not hold.
    pub(crate) fn of_graph(
        library: &'a Library,
        decls: &'a BTreeMap<CompoundId, CompoundDecl>,
        graph: GraphId,
    ) -> Self {
        let owner = match graph {
            GraphId::Main => None,
            GraphId::Compound(id) => {
                Some(decls.get(&id).expect("a scope's compound is in the set"))
            }
        };
        Self {
            library,
            decls,
            owner,
        }
    }

    pub const fn library(&self) -> &'a Library {
        self.library
    }

    /// The compound whose graph this is, or `None` for the main graph.
    pub const fn owner(&self) -> Option<&'a CompoundDecl> {
        self.owner
    }

    /// What `node`'s title reads: its own name, else its declaration's, as Blender's group node
    /// shows its group's name while its label is empty, so a renamed func or compound reads
    /// renamed on each node nobody named; empty for a node that resolves to nothing here.
    pub fn title(self, node: &'a Node) -> &'a str {
        match &node.name {
            Some(name) => name,
            None => self.resolve(node).map_or("", NodeDecl::name),
        }
    }

    /// What `node` declares: a library func, a special node's built-in, or the ports a
    /// compound's interface gives its instances and boundary nodes.
    ///
    /// # Panics
    ///
    /// If this scope does not resolve the node. A graph holds only nodes that
    /// resolve — [`GraphSet::reconcile`](crate::GraphSet::reconcile) keeps the
    /// rest as ghost nodes at load — so a miss is a broken contract; a node that may not
    /// resolve is asked through [`Self::resolve`].
    pub fn decl(&self, node: &Node) -> NodeDecl<'a> {
        match self.resolve(node) {
            Ok(decl) => decl,
            Err(reason) => panic!(
                "a graph node resolves, and {:?} does not: {reason}",
                node.name
            ),
        }
    }

    /// What `node` declares, or why it declares nothing here: no func or compound under its id,
    /// or a boundary node outside a compound. A declaration whose ports changed since the node
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
            NodeKind::Instance { compound, .. } => self
                .decls
                .get(&compound)
                .map(NodeDecl::Instance)
                .ok_or(Unresolved::MissingCompound),
            NodeKind::Boundary(side) => self
                .owner
                .map(|owner| NodeDecl::Boundary { side, owner })
                .ok_or(Unresolved::NoInterface),
        }
    }
}
