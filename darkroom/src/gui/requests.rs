//! The frame's pending requests: everything a UI surface asked for, in the
//! order it asked.

use scenarium::NodeId;

use crate::core::document::dock::DockOp;
use crate::core::edit::intent::types::GraphIntent;
use crate::gui::app::commands::AppCommand;

/// One thing a UI surface asked for, tagged by who owns the state it touches.
/// The tier says which level drains it, and that is the whole of the
/// difference between them:
///
///   - [`Graph`](Self::Graph) — the document's graph. Drained by the editor:
///     validated, applied, and recorded as an undo step; flips the unsaved
///     flag.
///   - [`View`](Self::View) — the pane arrangement around it. Drained by the
///     editor too, applied in place, recording nothing and dirtying nothing,
///     so Ctrl+Z walks past a tab switch to the last graph edit.
///   - [`App`](Self::App) — state the editor does not own. Left queued by the
///     editor's drain and taken by `App` once the pass is over: every one
///     needs `&mut App`, a blocking dialog, or both.
///
/// A surface picks the tier by what it is asking for, never by where it sits
/// or when it runs — the menu bar raises all three.
#[derive(Debug)]
pub(crate) enum Request {
    Graph(GraphIntent),
    View(DockOp),
    App(AppCommand),
}

/// A request the editor applies itself: the two halves of the document, the
/// graph and the layout around it. What [`Requests::drain_document`] yields,
/// so the commit path matches exhaustively over exactly what can reach it
/// rather than carrying an arm for a tier it never sees.
#[derive(Debug)]
pub(crate) enum DocumentRequest {
    Graph(GraphIntent),
    View(DockOp),
}

/// A frame's requests, in the order they were raised.
///
/// One queue for all three tiers, so a surface pushes and moves on rather than
/// knowing which level will pick its request up. Nothing is dropped and
/// nothing is reordered: two surfaces answering the same frame both get what
/// they asked for, in the order the frame produced them.
///
/// Each owner drains its own tier and leaves the rest alone —
/// [`Self::drain_document`] for the editor, [`Self::drain_app`] for the shell
/// — so a request is never copied out into a side buffer on the way to
/// whoever runs it.
#[derive(Debug, Default)]
pub(crate) struct Requests {
    items: Vec<Request>,
}

impl Requests {
    /// Queue a graph edit.
    pub(crate) fn push_graph(&mut self, intent: GraphIntent) {
        self.items.push(Request::Graph(intent));
    }

    /// Queue every graph edit `iter` yields.
    pub(crate) fn extend_graph(&mut self, iter: impl IntoIterator<Item = GraphIntent>) {
        self.items.extend(iter.into_iter().map(Request::Graph));
    }

    /// Queue the removal of every node `nodes` yields — one
    /// [`GraphIntent::RemoveNode`] each, which the drain batches into a single
    /// undo entry. Shared by the Delete/Backspace chord, the node context
    /// menu's "Remove", and the breaker's multi-delete.
    ///
    /// Takes an iterator rather than a slice because the selection-driven
    /// callers hold a `BTreeSet` and the breaker a `Vec`; a slice would make
    /// the first two allocate one just to call this.
    pub(crate) fn push_node_removals(&mut self, nodes: impl IntoIterator<Item = NodeId>) {
        self.extend_graph(
            nodes
                .into_iter()
                .map(|node_id| GraphIntent::RemoveNode { node_id }),
        );
    }

    /// Queue a mutation of the pane arrangement.
    pub(crate) fn push_view(&mut self, op: DockOp) {
        self.items.push(Request::View(op));
    }

    /// Queue a side effect for `App` to run after the pass.
    pub(crate) fn push_app(&mut self, command: AppCommand) {
        self.items.push(Request::App(command));
    }

    /// Take everything the document owns, in the order raised, leaving the
    /// app tier queued for [`Self::drain_app`].
    ///
    /// The editor calls this three times a frame — after the navigation scan,
    /// after the prepass, and after the record — so a request raised in one
    /// phase lands before the next reads the document. App commands survive
    /// every one of them and come out at the end, which is the only time
    /// there is an `&mut App` to run them with.
    pub(crate) fn drain_document(&mut self) -> impl Iterator<Item = DocumentRequest> + '_ {
        self.items
            .extract_if(.., |item| !matches!(item, Request::App(_)))
            .map(|item| match item {
                Request::Graph(intent) => DocumentRequest::Graph(intent),
                Request::View(op) => DocumentRequest::View(op),
                Request::App(_) => unreachable!("the predicate above kept the app tier queued"),
            })
    }

    /// Take the app tier, in the order raised.
    pub(crate) fn drain_app(&mut self) -> impl Iterator<Item = AppCommand> + '_ {
        self.items
            .extract_if(.., |item| matches!(item, Request::App(_)))
            .map(|item| match item {
                Request::App(command) => command,
                _ => unreachable!("the predicate above kept everything else queued"),
            })
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub(crate) fn clear(&mut self) {
        self.items.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::document::TabRef;
    use crate::gui::app::commands::run::RunCommand;

    fn raise() -> GraphIntent {
        GraphIntent::RemoveNode {
            node_id: NodeId::unique(),
        }
    }

    fn close_prefs() -> DockOp {
        DockOp::CloseTab {
            tab: TabRef::Preferences,
        }
    }

    /// The document drain takes both of its tiers in the order raised and
    /// steps over the app tier without disturbing it — which is what lets the
    /// editor drain three times a frame while `App` still gets every command,
    /// in order, once the pass is over.
    #[test]
    fn each_level_drains_its_own_tier_and_leaves_the_rest_queued() {
        let mut out = Requests::default();
        out.push_graph(raise());
        out.push_app(AppCommand::Run(RunCommand::Once));
        out.push_view(close_prefs());
        out.push_app(AppCommand::Quit);
        out.extend_graph([raise(), raise()]);

        let first: Vec<&str> = out
            .drain_document()
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
        assert!(!out.is_empty(), "the app tier is still queued");

        // A second document drain — the editor runs three a frame — finds
        // nothing left of its own and still leaves the app tier alone.
        assert_eq!(out.drain_document().count(), 0);

        let commands: Vec<AppCommand> = out.drain_app().collect();
        assert!(
            matches!(
                commands[..],
                [AppCommand::Run(RunCommand::Once), AppCommand::Quit]
            ),
            "the app tier comes out in the order raised: {commands:?}"
        );
        assert!(
            out.is_empty(),
            "and the queue is empty once both have drained"
        );
    }
}
