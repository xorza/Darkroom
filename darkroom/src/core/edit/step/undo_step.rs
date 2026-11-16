//! The undo history's whole vocabulary.

use scenarium::GraphId;
use serde::{Deserialize, Serialize};

use crate::core::document::Document;
use crate::core::edit::edit_facts::{EditFacts, Touched};
use crate::core::edit::step::change::Direction;
use crate::core::edit::step::graph_step::GraphStep;
use crate::core::edit::step::set_definition::SetDefinition;
use crate::core::edit::step::set_interface::SetInterface;
use crate::core::edit::step::set_side::SetSide;
use crate::core::edit::step::set_subgraph::SetSubgraph;

/// One self-contained entry of the undo history: an edit of one graph of the document and its
/// view, or of the set of graphs, a subgraph's definition or its interface.
///
/// Only edits of the document's graphs are undoable: pane arrangement records nothing, so Ctrl+Z
/// walks past a tab switch to the last edit that changed a graph.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) enum UndoStep {
    Graph {
        graph: GraphId,
        step: GraphStep,
    },
    /// Boxed: a whole definition is several times the size of every other step.
    Subgraph(Box<SetSubgraph>),
    Interface(Box<SetInterface>),
    Side(SetSide),
    /// Boxed: two whole definitions.
    Definition(Box<SetDefinition>),
}

impl UndoStep {
    /// Write the "to" half to `document`: the initial commit, and redo of a stored step.
    pub(crate) fn apply(&self, document: &mut Document) {
        self.write(document, Direction::Forward);
    }

    /// Write the "from" half to `document`, restoring the state before [`Self::apply`].
    pub(crate) fn revert(&self, document: &mut Document) {
        self.write(document, Direction::Backward);
    }

    fn write(&self, document: &mut Document, dir: Direction) {
        match self {
            Self::Graph { graph, step } => step.kind().write(document, *graph, dir),
            Self::Subgraph(step) => step.write(document, dir),
            Self::Interface(step) => step.write(document, dir),
            Self::Side(step) => step.write(document, dir),
            Self::Definition(step) => step.write(document, dir),
        }
    }

    pub(crate) fn is_noop(&self) -> bool {
        match self {
            Self::Graph { step, .. } => step.kind().is_noop(),
            // A definition always comes or goes.
            Self::Subgraph(_) => false,
            Self::Interface(step) => step.is_noop(),
            Self::Side(step) => step.is_noop(),
            Self::Definition(step) => step.is_noop(),
        }
    }

    pub(crate) fn dirties_document(&self) -> bool {
        match self {
            Self::Graph { step, .. } => step.kind().dirties_document(),
            Self::Subgraph(_) | Self::Interface(_) | Self::Side(_) | Self::Definition(_) => true,
        }
    }

    /// What the step changes, and the graphs it changes.
    pub(crate) fn facts(&self) -> EditFacts {
        match self {
            Self::Graph { graph, step } => EditFacts {
                touched: Touched::Graph(*graph),
                ..step.kind().facts()
            },
            Self::Subgraph(step) => step.facts(),
            Self::Interface(step) => step.facts(),
            // A side holds no node and no port.
            Self::Side(_) => EditFacts::NONE,
            Self::Definition(step) => step.facts(),
        }
    }

    /// Whether `next`, a later step of the same held gesture, is a frame of this one, which
    /// [`Self::absorb`] folds in: a step of the same kind on the same graph.
    pub(crate) fn absorbs(&self, next: &Self) -> bool {
        match (self, next) {
            (
                Self::Graph { graph, step },
                Self::Graph {
                    graph: next_graph,
                    step: next,
                },
            ) => graph == next_graph && step.absorbs(next),
            _ => false,
        }
    }

    /// Fold `next`, a later frame of the same held gesture, into this step: keep this step's
    /// "from" half and adopt `next`'s "to" half, in place.
    ///
    /// # Panics
    ///
    /// If the two are not frames of one gesture on one graph. A gesture emits one kind of intent
    /// on one graph for its whole life, so a mismatch is the caller's bug.
    pub(crate) fn absorb(&mut self, next: &Self) {
        let (
            Self::Graph { graph, step },
            Self::Graph {
                graph: next_graph,
                step: next,
            },
        ) = (self, next)
        else {
            panic!("a gesture folds frames of one graph's edits");
        };
        assert_eq!(graph, next_graph, "a gesture folds frames of one graph");
        step.absorb(next);
    }
}
