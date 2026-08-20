//! The undo stack's whole vocabulary.

use serde::{Deserialize, Serialize};

use crate::core::document::Document;
use crate::core::edit::step::change::Direction;
use crate::core::edit::step::gesture_key::GestureKey;
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
    NodePresence(NodePresence),
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

    pub(crate) fn gesture_key(&self) -> Option<GestureKey> {
        self.kind().gesture_key()
    }

    /// Fold a consecutive step of the same gesture into this one — see
    /// [`Reversible::coalesce`].
    pub(crate) fn coalesce(&self, next: &Self) -> Option<Self> {
        self.kind().coalesce(next)
    }

    /// The payload behind the variant, as the behaviour it implements.
    ///
    /// The single exhaustive match over this enum, and the reason there is
    /// only one: everything else the pipeline asks a step is answered by the
    /// kind's own `impl`, so a new variant adds a line here and a file of its
    /// own rather than an arm in each of six matches.
    fn kind(&self) -> &dyn Reversible {
        match self {
            Self::NodePresence(step) => step,
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
