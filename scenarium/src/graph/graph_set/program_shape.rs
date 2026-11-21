//! [`ProgramShape`]: what a graph expands into, held to the bounds every graph keeps.

use hashbrown::HashMap;

use crate::graph::Graph;
use crate::graph::node::NodeKind;
use crate::identity::CompoundId;
use crate::identity::program_id::ProgramId;

/// What a graph expands into: how deep its program paths run and how many program nodes it
/// makes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProgramShape {
    /// The longest program path among its nodes: 1 for a graph of funcs alone, one more for each
    /// instance a node lies inside.
    pub levels: usize,
    pub node_count: usize,
}

impl ProgramShape {
    /// The deepest a graph nests: a [`ProgramId`] holds this many ids.
    pub const MAX_LEVELS: usize = ProgramId::MAX_LEN;
    /// The most program nodes a graph expands into: 2¹⁶, past every graph a person wires by hand
    /// yet small enough that a blowup stays a refused edit, not a compile that runs out of
    /// memory.
    pub const MAX_NODES: usize = 1 << 16;

    /// What one node of kind `kind` adds, given the shape of every compound it may place.
    pub(super) fn of_node(kind: &NodeKind, shapes: &HashMap<CompoundId, Self>) -> Self {
        match kind {
            NodeKind::Body { .. } => Self {
                levels: 1,
                node_count: 1,
            },
            NodeKind::Instance { compound, .. } => {
                let inner = shapes
                    .get(compound)
                    .expect("a placed compound's shape is known before its instance's");
                Self {
                    levels: inner.levels + 1,
                    node_count: inner.node_count,
                }
            }
            NodeKind::Boundary(_) => Self::default(),
        }
    }

    /// This shape with `other` beside it in one graph.
    pub(super) fn beside(self, other: Self) -> Self {
        Self {
            levels: self.levels.max(other.levels),
            node_count: self.node_count.saturating_add(other.node_count),
        }
    }

    /// What `graph` expands into, given the shape of every compound it places.
    pub(super) fn of_graph(graph: &Graph, shapes: &HashMap<CompoundId, Self>) -> Self {
        graph.iter().fold(Self::default(), |shape, node| {
            shape.beside(Self::of_node(&node.kind, shapes))
        })
    }
}
