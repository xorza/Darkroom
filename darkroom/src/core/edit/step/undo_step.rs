//! The undo stack's whole vocabulary.

use serde::{Deserialize, Serialize};

use crate::core::document::Document;
use crate::core::edit::step::change::Direction;
use crate::core::edit::step::move_selection::MoveSelection;
use crate::core::edit::step::node_presence::NodePresence;
use crate::core::edit::step::raise::Raise;
use crate::core::edit::step::rename_node::RenameNode;
use crate::core::edit::step::reversible::Reversible;
use crate::core::edit::step::set_input::SetInput;
use crate::core::edit::step::set_node_property::SetNodeProperty;
use crate::core::edit::step::set_selection::SetSelection;
use crate::core::edit::step::set_subscription::SetSubscription;
use crate::core::edit::step::set_viewport::SetViewport;

/// One self-contained entry of the undo history: which kind of edit it is,
/// and that kind's payload — the slot it touches with both halves of what
/// went into it.
///
/// The variants are *primitives*, not a mirror of the
/// [`GraphIntent`](crate::core::edit::graph_intent::GraphIntent) vocabulary
/// that produces them: one intent can lower to several steps, and two intents
/// that are each other's inverse (add a node, remove a node) lower to one
/// kind. Nothing here has to be kept in step with anything there.
///
/// Only graph edits are undoable: pane arrangement applies straight to the
/// layout and records nothing, so Ctrl+Z walks past a tab switch to the last
/// edit that changed the graph. That is why this is the whole step
/// vocabulary rather than one arm of a wider one.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) enum UndoStep {
    /// Boxed: a whole detached node is several times the size of every other step.
    NodePresence(Box<NodePresence>),
    MoveSelection(MoveSelection),
    RenameNode(RenameNode),
    SetInput(SetInput),
    SetSelection(SetSelection),
    Raise(Raise),
    SetNodeProperty(SetNodeProperty),
    SetViewport(SetViewport),
    SetSubscription(SetSubscription),
}

impl UndoStep {
    /// Forward apply: write the "to" half to `doc`. Used by the initial
    /// commit and by undo-stack redo, which replays a stored step without
    /// rebuilding it.
    pub(crate) fn apply(&self, doc: &mut Document) {
        self.kind().write(doc, Direction::Forward);
    }

    /// Backward apply: write the "from" half to `doc`. Calling this after
    /// [`Self::apply`] restores the document to its pre-commit state.
    pub(crate) fn revert(&self, doc: &mut Document) {
        self.kind().write(doc, Direction::Backward);
    }

    pub(crate) fn is_noop(&self) -> bool {
        self.kind().is_noop()
    }

    pub(crate) fn dirties_document(&self) -> bool {
        self.kind().dirties_document()
    }

    pub(crate) fn invalidates_cached_geometry(&self) -> bool {
        self.kind().invalidates_cached_geometry()
    }

    pub(crate) fn retypes_outputs(&self) -> bool {
        self.kind().retypes_outputs()
    }

    /// Fold `next`, a later frame of the same held gesture, into this step:
    /// keep this step's "from" half and adopt `next`'s "to" half, in place.
    ///
    /// # Panics
    ///
    /// If the two are not the same kind of gesture step. A gesture emits one
    /// kind of intent for its whole life, so a mismatch is the caller's bug.
    pub(crate) fn absorb(&mut self, next: &Self) {
        match (self, next) {
            (Self::MoveSelection(open), Self::MoveSelection(next)) => open.absorb(next),
            (Self::SetViewport(open), Self::SetViewport(next)) => open.absorb(next),
            (open, next) => panic!("a gesture cannot fold {next:?} into {open:?}"),
        }
    }

    /// The payload behind the variant, as the behaviour it implements.
    ///
    /// The one match that hands out a payload: everything the pipeline asks a
    /// single step is answered by the kind's own `impl`, so a new variant adds
    /// a line here and a file of its own rather than an arm in each of five
    /// matches. [`Self::absorb`] matches too, as it pairs two payloads.
    fn kind(&self) -> &dyn Reversible {
        match self {
            Self::NodePresence(step) => step.as_ref(),
            Self::MoveSelection(step) => step,
            Self::RenameNode(step) => step,
            Self::SetInput(step) => step,
            Self::SetSelection(step) => step,
            Self::Raise(step) => step,
            Self::SetNodeProperty(step) => step,
            Self::SetViewport(step) => step,
            Self::SetSubscription(step) => step,
        }
    }
}
