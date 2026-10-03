//! The frame's pending requests: everything a UI surface asked for, in the
//! order it asked.

use std::collections::VecDeque;

use palantir::DockOp;

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
    pub(crate) fn open_gesture(&mut self) -> GestureId {
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
    pub(crate) fn push_view(&mut self, op: DockOp<TabRef>) {
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
    pub(crate) fn document(&mut self) -> &mut DocumentQueue {
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
mod tests {
    use scenarium::NodeId;
    use std::iter;

    use super::*;
    use crate::core::edit::document_request::DocumentRequest;
    use crate::gui::app::commands::run::RunCommand;

    fn remove_node() -> GraphIntent {
        GraphIntent::RemoveNode {
            node_id: NodeId::unique(),
        }
    }

    fn close_prefs() -> DockOp<TabRef> {
        DockOp::CloseTab {
            tab: TabRef::Preferences,
        }
    }

    /// The document drain takes both of its tiers in the order raised and
    /// leaves the app tier untouched — which is what lets the editor drain
    /// three times a frame while `App` still gets every command, in order,
    /// once the pass is over. Pinned here because the routing lives in the
    /// push methods now: a tier pushed to the wrong queue would surface as a
    /// command vanishing mid-pass rather than as a type error.
    #[test]
    fn each_level_drains_its_own_tier_and_leaves_the_rest_queued() {
        let mut out = Requests::default();
        out.push_graph(remove_node());
        out.push_app(AppCommand::Run(RunCommand::Once));
        out.push_view(close_prefs());
        out.push_app(AppCommand::Quit);
        out.extend_graph([remove_node(), remove_node()]);

        let first: Vec<&str> = out
            .document()
            .drain()
            .map(|item| match item {
                DocumentRequest::Graph(_) => "graph",
                DocumentRequest::View(_) => "view",
            })
            .collect();
        assert_eq!(
            first,
            ["graph", "view", "graph", "graph"],
            "both document tiers come out interleaved as raised"
        );
        // A second document drain — the editor runs three a frame — finds
        // nothing left of its own and still leaves the app tier alone.
        assert_eq!(out.document().drain().count(), 0);

        let commands: Vec<AppCommand> = iter::from_fn(|| out.pop_app()).collect();
        assert!(
            matches!(
                commands[..],
                [AppCommand::Run(RunCommand::Once), AppCommand::Quit]
            ),
            "the app tier comes out in the order raised: {commands:?}"
        );
        assert!(
            out.document().is_empty(),
            "and the queue is empty once both have drained"
        );
    }
}
