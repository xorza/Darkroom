//! [`EditFacts`]: what a landed edit changed of the document's shape.

use std::ops::{BitOr, BitOrAssign};

use scenarium::GraphId;

/// What an edit, its undo or its redo changed that a frontend lays its view out by: facts about
/// the document, which a frontend maps onto its own caches. A move, a reorder or a selection
/// changes none of them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct EditFacts {
    /// A node or a stub came or went.
    pub(crate) node_set: bool,
    /// A node's name changed.
    pub(crate) renamed: bool,
    /// An input changed between a const and a wire or nothing.
    pub(crate) binding_kind: bool,
    /// Which graphs the edit wrote.
    pub(crate) touched: Touched,
}

/// Which graphs a batch of edits wrote, folded with no allocation: one graph by name, or more
/// than one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Touched {
    #[default]
    Nothing,
    Graph(GraphId),
    Several,
}

impl Touched {
    /// Whether an edit of this reach wrote `graph`: `Several` may have.
    pub(crate) fn reaches(self, graph: GraphId) -> bool {
        match self {
            Self::Nothing => false,
            Self::Graph(touched) => touched == graph,
            Self::Several => true,
        }
    }
}

impl BitOr for Touched {
    type Output = Self;

    /// What either batch wrote: the same graph twice is that graph, two graphs are several.
    fn bitor(self, rhs: Self) -> Self {
        match (self, rhs) {
            (Self::Nothing, other) | (other, Self::Nothing) => other,
            (Self::Graph(a), Self::Graph(b)) if a == b => self,
            _ => Self::Several,
        }
    }
}

impl EditFacts {
    pub(crate) const NONE: Self = Self {
        node_set: false,
        renamed: false,
        binding_kind: false,
        touched: Touched::Nothing,
    };

    /// Whether this edit leaves a frontend's cached geometry behind. A node that came has no cached
    /// port offsets, and a rename or a const editor shown or hidden changes a measured size; a
    /// move or a reorder changes neither, as a port center is `node.pos + cached offset`. A
    /// spurious relayout on a move is costly: a node drag emits one step per gesture frame.
    pub(crate) const fn strands_geometry(self) -> bool {
        self.node_set || self.renamed || self.binding_kind
    }
}

impl BitOr for EditFacts {
    type Output = Self;

    /// What either batch changed.
    fn bitor(self, rhs: Self) -> Self {
        Self {
            node_set: self.node_set || rhs.node_set,
            renamed: self.renamed || rhs.renamed,
            binding_kind: self.binding_kind || rhs.binding_kind,
            touched: self.touched | rhs.touched,
        }
    }
}

impl BitOrAssign for EditFacts {
    fn bitor_assign(&mut self, rhs: Self) {
        *self = *self | rhs;
    }
}

#[cfg(test)]
mod tests {
    use scenarium::{GraphId, SubgraphId};

    use crate::core::edit::edit_facts::{EditFacts, Touched};

    /// The fold of the graphs a batch wrote, every pair: nothing yields the other side, one graph
    /// twice is that graph, two graphs or several with anything are several. And what each reaches.
    #[test]
    fn touched_folds_to_the_one_graph_or_several() {
        let (main, sub) = (
            Touched::Graph(GraphId::Main),
            Touched::Graph(GraphId::Subgraph(SubgraphId::from_u128(1))),
        );
        let cases = [
            (Touched::Nothing, Touched::Nothing, Touched::Nothing),
            (Touched::Nothing, main, main),
            (main, Touched::Nothing, main),
            (main, main, main),
            (main, sub, Touched::Several),
            (sub, main, Touched::Several),
            (Touched::Several, Touched::Nothing, Touched::Several),
            (Touched::Nothing, Touched::Several, Touched::Several),
            (Touched::Several, main, Touched::Several),
        ];
        for (a, b, folded) in cases {
            assert_eq!(a | b, folded, "{a:?} | {b:?}");
        }
        assert!(main.reaches(GraphId::Main));
        assert!(!sub.reaches(GraphId::Main));
        assert!(Touched::Several.reaches(GraphId::Main));
        assert!(!Touched::Nothing.reaches(GraphId::Main));
    }

    /// Each fact alone strands the geometry, and an edit with none, a move, does not.
    #[test]
    fn each_shape_fact_strands_the_geometry() {
        for (facts, strands) in [
            (EditFacts::NONE, false),
            (
                EditFacts {
                    node_set: true,
                    ..EditFacts::NONE
                },
                true,
            ),
            (
                EditFacts {
                    renamed: true,
                    ..EditFacts::NONE
                },
                true,
            ),
            (
                EditFacts {
                    binding_kind: true,
                    ..EditFacts::NONE
                },
                true,
            ),
        ] {
            assert_eq!(facts.strands_geometry(), strands, "{facts:?}");
        }
    }
}
