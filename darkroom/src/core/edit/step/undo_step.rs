//! The undo history's whole vocabulary.

use scenarium::GraphId;
use serde::{Deserialize, Serialize};

use crate::core::document::Document;
use crate::core::edit::edit_facts::{EditFacts, Touched};
use crate::core::edit::step::change::Direction;
use crate::core::edit::step::graph_step::GraphStep;

/// One self-contained entry of the undo history: an edit of one graph of the document and its
/// view.
///
/// Only edits of the document's graphs are undoable: pane arrangement records nothing, so Ctrl+Z
/// walks past a tab switch to the last edit that changed a graph.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) enum UndoStep {
    Graph { graph: GraphId, step: GraphStep },
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
        let Self::Graph { graph, step } = self;
        step.kind().write(document, *graph, dir);
    }

    pub(crate) fn is_noop(&self) -> bool {
        let Self::Graph { step, .. } = self;
        step.kind().is_noop()
    }

    pub(crate) fn dirties_document(&self) -> bool {
        let Self::Graph { step, .. } = self;
        step.kind().dirties_document()
    }

    /// What the step changes, and the graphs it changes.
    pub(crate) fn facts(&self) -> EditFacts {
        let Self::Graph { graph, step } = self;
        EditFacts {
            touched: Touched::Graph(*graph),
            ..step.kind().facts()
        }
    }

    /// Whether `next`, a later step of the same held gesture, is a frame of this one, which
    /// [`Self::absorb`] folds in: a step of the same kind on the same graph.
    pub(crate) fn absorbs(&self, next: &Self) -> bool {
        let (
            Self::Graph { graph, step },
            Self::Graph {
                graph: next_graph,
                step: next,
            },
        ) = (self, next);
        graph == next_graph && step.absorbs(next)
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
        ) = (self, next);
        assert_eq!(graph, next_graph, "a gesture folds frames of one graph");
        step.absorb(next);
    }
}
