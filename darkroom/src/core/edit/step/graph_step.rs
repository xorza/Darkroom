//! [`GraphStep`]: an edit of one graph and its view.

use serde::{Deserialize, Serialize};

use crate::core::edit::step::reversible::Reversible;
use crate::core::edit::step::set_depth::SetDepth;
use crate::core::edit::step::set_input::SetInput;
use crate::core::edit::step::set_node::SetNode;
use crate::core::edit::step::set_node_name::SetNodeName;
use crate::core::edit::step::set_node_property::SetNodeProperty;
use crate::core::edit::step::set_positions::SetPositions;
use crate::core::edit::step::set_selection::SetSelection;
use crate::core::edit::step::set_subscription::SetSubscription;
use crate::core::edit::step::set_ties::SetTies;

/// One edit of a graph and its view: which kind of edit it is, and that kind's payload, the slot
/// it touches with both halves of what went into it. Which graph is the
/// [`UndoStep`](crate::core::edit::step::undo_step::UndoStep)'s.
///
/// The variants are *primitives*, not a mirror of the
/// [`GraphIntent`](crate::core::edit::graph_intent::GraphIntent) vocabulary: one intent can lower
/// to several steps, and two intents that are each other's inverse (add a node, remove a node)
/// lower to one kind.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) enum GraphStep {
    /// Boxed: a whole detached node is several times the size of every other step.
    Node(Box<SetNode>),
    Positions(SetPositions),
    NodeName(SetNodeName),
    /// Boxed: each half holds a binding or a ghost of one, with the ports it kept.
    Input(Box<SetInput>),
    Selection(SetSelection),
    Depth(SetDepth),
    NodeProperty(SetNodeProperty),
    Subscription(SetSubscription),
    Ties(SetTies),
}

impl GraphStep {
    /// Whether `next`, a later step of the same held gesture, is a frame of this one, which
    /// [`Self::absorb`] folds in: a move after a move, a tie edit after a tie edit.
    pub(super) const fn absorbs(&self, next: &Self) -> bool {
        matches!(
            (self, next),
            (Self::Positions(_), Self::Positions(_)) | (Self::Ties(_), Self::Ties(_))
        )
    }

    /// Fold `next`, a later frame of the same held gesture, into this step:
    /// keep this step's "from" half and adopt `next`'s "to" half, in place.
    ///
    /// # Panics
    ///
    /// If the two are not the same kind of gesture step. A gesture emits one
    /// kind of intent for its whole life, so a mismatch is the caller's bug.
    pub(super) fn absorb(&mut self, next: &Self) {
        match (self, next) {
            (Self::Positions(open), Self::Positions(next)) => open.absorb(next),
            (Self::Ties(open), Self::Ties(next)) => open.absorb(next),
            (open, next) => panic!("a gesture cannot fold {next:?} into {open:?}"),
        }
    }

    /// The payload behind the variant, as the behaviour it implements. The one match that hands
    /// out a payload, so a new variant adds a line here and a file of its own; [`Self::absorb`]
    /// matches too, as it pairs two payloads.
    pub(super) fn kind(&self) -> &dyn Reversible {
        match self {
            Self::Node(step) => step.as_ref(),
            Self::Positions(step) => step,
            Self::NodeName(step) => step,
            Self::Input(step) => step.as_ref(),
            Self::Selection(step) => step,
            Self::Depth(step) => step,
            Self::NodeProperty(step) => step,
            Self::Subscription(step) => step,
            Self::Ties(step) => step,
        }
    }
}
