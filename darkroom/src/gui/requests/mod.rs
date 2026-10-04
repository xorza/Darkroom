//! The frame's pending requests: everything a UI surface asked for, in the
//! order it asked.

use std::collections::VecDeque;

use palantir::DockOperation;

use crate::core::document::TabRef;
use crate::core::edit::document_queue::DocumentQueue;
use crate::core::edit::gesture_id::GestureId;
use crate::core::edit::graph_intent::GraphIntent;
use crate::gui::app::commands::AppCommand;

/// A frame's requests, in the order they were raised.
///
/// A surface pushes and moves on: the push methods are one vocabulary and say
/// nothing about which level will pick the request up. Behind them the tiers
/// are stored apart, one queue per taker, so each owner takes its own by type
/// — [`Self::document`] for the open document, [`Self::pop_app`] for the
/// shell — and neither has to step over, or pattern-match away, a tier its
/// queue cannot hold.
///
/// The document tier is a graph edit or a dock op; see
/// [`DocumentRequest`](crate::core::edit::document_request::DocumentRequest).
/// The app tier is an [`AppCommand`], state the document does not own: left
/// queued by the document's drain and taken by `App` once the pass is over,
/// because every one needs `&mut App`, a blocking dialog, or both. A surface
/// picks the tier by what it is asking for, never by where it sits or when it
/// runs — the menu bar raises all three.
///
/// Nothing is dropped and nothing is reordered *within* a tier: two surfaces
/// answering the same frame both get what they asked for, in the order the
/// frame produced them. Across tiers there is no order to keep, since the app
/// tier runs after the whole pass rather than interleaved with the document's.
#[derive(Debug, Default)]
pub(crate) struct Requests {
    document: DocumentQueue,
    app: VecDeque<AppCommand>,
}

impl Requests {
    /// A fresh id for a gesture that starts now; see [`GestureId`].
    pub(crate) const fn open_gesture(&mut self) -> GestureId {
        self.document.open_gesture()
    }

    /// Queue a graph edit.
    pub(crate) fn push_graph(&mut self, intent: GraphIntent) {
        self.document.push_graph(intent);
    }

    /// Queue every graph edit `iter` yields.
    pub(crate) fn extend_graph(&mut self, iter: impl IntoIterator<Item = GraphIntent>) {
        self.document.extend_graph(iter);
    }

    /// Queue a mutation of the pane arrangement.
    pub(crate) fn push_view(&mut self, op: DockOperation<TabRef>) {
        self.document.push_view(op);
    }

    /// Queue a side effect for `App` to run after the pass.
    pub(crate) fn push_app(&mut self, command: AppCommand) {
        self.app.push_back(command);
    }

    /// The document's tier, for its drain. The app tier stays queued for
    /// [`Self::pop_app`]: the document drains three times a frame, and app
    /// commands come out at the end, the only time there is an `&mut App` to
    /// run them with.
    pub(crate) const fn document(&mut self) -> &mut DocumentQueue {
        &mut self.document
    }

    /// Take the next app-tier command, in the order raised.
    ///
    /// One at a time rather than an iterator over the lot, because running a
    /// command needs `&mut App` and the queue lives on it: an iterator would
    /// hold the queue borrowed for the whole loop, forcing the caller to move
    /// the commands somewhere else first. Popping ends the borrow before each
    /// dispatch, so the loop needs no buffer and allocates nothing.
    ///
    /// A `VecDeque` for that reason — `Vec::remove(0)` would be O(n) per
    /// command, and popping the *back* would run them in reverse.
    pub(crate) fn pop_app(&mut self) -> Option<AppCommand> {
        self.app.pop_front()
    }

    pub(crate) fn clear(&mut self) {
        self.document.clear();
        self.app.clear();
    }
}

#[cfg(test)]
mod tests;
