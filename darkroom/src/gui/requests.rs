//! The frame's pending requests: everything a UI surface asked for, in the
//! order it asked.

use scenarium::NodeId;

use crate::core::document::dock::DockOp;
use crate::core::edit::intent::types::GraphIntent;
use crate::gui::app::commands::AppCommand;

/// One thing a UI surface asked for, tagged by who owns the state it touches.
/// The tier is the whole of the difference between them — it decides what the
/// drain does, and nothing else does:
///
///   - [`Graph`](Self::Graph) — the document's graph. Validated, applied, and
///     recorded as an undo step; flips the unsaved flag.
///   - [`View`](Self::View) — the pane arrangement around it. Applied in
///     place, recording nothing and dirtying nothing, so Ctrl+Z walks past a
///     tab switch to the last graph edit.
///   - [`App`](Self::App) — state the editor does not own. Collected and
///     handed back to `App`, which runs it once the pass is over: every one
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

/// A frame's requests, in the order they were raised.
///
/// One queue for all three tiers, so a surface pushes and moves on rather than
/// knowing which drain will pick its request up. Nothing is dropped and
/// nothing is reordered: two surfaces answering the same frame both get what
/// they asked for, in the order the frame produced them.
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

    pub(crate) fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Take everything queued, leaving the buffer empty (and its capacity
    /// intact for the next frame).
    pub(crate) fn drain(&mut self) -> std::vec::Drain<'_, Request> {
        self.items.drain(..)
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

    /// The sink is a queue, not a set or a slot: requests drain in the order
    /// they were raised, whatever tier each belongs to, and a second request
    /// in a tier never displaces the first.
    #[test]
    fn drains_every_tier_in_the_order_raised() {
        let mut out = Requests::default();
        out.push_graph(raise());
        out.extend_graph([raise(), raise()]);
        out.push_view(DockOp::CloseTab {
            tab: TabRef::Preferences,
        });
        out.push_app(AppCommand::Run(RunCommand::Once));
        out.push_app(AppCommand::Quit);
        out.push_graph(raise());

        let tiers: Vec<&str> = out
            .drain()
            .map(|item| match item {
                Request::Graph(_) => "graph",
                Request::View(_) => "view",
                Request::App(_) => "app",
            })
            .collect();
        assert_eq!(
            tiers,
            ["graph", "graph", "graph", "view", "app", "app", "graph"]
        );
        assert!(out.is_empty(), "draining empties the buffer");
    }
}
