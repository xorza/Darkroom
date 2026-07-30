//! The frame's pending mutations, each tagged with the graph it commits
//! against.

use scenarium::NodeId;

use crate::core::document::dock::DockOp;
use crate::core::edit::intent::types::Intent;

/// One queued mutation: an [`Intent`] against the graph, or a [`DockOp`]
/// against the layout around it.
#[derive(Debug)]
pub(crate) enum Queued {
    Scoped(Intent),
    Dock(DockOp),
}

/// A frame's queued mutations, in the order they were raised.
///
/// A mutation of the graph is an [`Intent`]; one of the layout is a
/// [`DockOp`], and [`Self::push_dock`] is the only door it fits through.
#[derive(Debug, Default)]
pub(crate) struct Intents {
    items: Vec<Queued>,
}

impl Intents {
    /// Queue `intent` against the graph.
    pub(crate) fn push(&mut self, intent: Intent) {
        self.items.push(Queued::Scoped(intent));
    }

    /// Queue every intent `iter` yields.
    pub(crate) fn extend(&mut self, iter: impl IntoIterator<Item = Intent>) {
        self.items.extend(iter.into_iter().map(Queued::Scoped));
    }

    /// Queue the removal of every node `nodes` yields — one
    /// [`Intent::RemoveNode`] each, which `drain_intents` batches into a
    /// single undo entry. Shared by the Delete/Backspace chord, the node
    /// context menu's "Remove", and the breaker's multi-delete.
    ///
    /// Takes an iterator rather than a slice because the selection-driven
    /// callers hold a `BTreeSet` and the breaker a `Vec`; a slice would make
    /// the first two allocate one just to call this.
    pub(crate) fn push_node_removals(&mut self, nodes: impl IntoIterator<Item = NodeId>) {
        self.extend(
            nodes
                .into_iter()
                .map(|node_id| Intent::RemoveNode { node_id }),
        );
    }

    /// Queue a mutation of the dock layout, raised from chrome that is not a
    /// canvas (a tab strip, a menu command, a keyboard chord).
    pub(crate) fn push_dock(&mut self, op: DockOp) {
        self.items.push(Queued::Dock(op));
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Take everything queued, leaving the buffer empty (and its capacity
    /// intact for the next frame).
    pub(crate) fn drain(&mut self) -> std::vec::Drain<'_, Queued> {
        self.items.drain(..)
    }

    pub(crate) fn clear(&mut self) {
        self.items.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::document::dock::DockOp;
    use crate::core::edit::intent::types::Intent;

    fn raise() -> Intent {
        Intent::RemoveNode {
            node_id: scenarium::NodeId::unique(),
        }
    }

    /// The sink is a queue, not a set: intents drain in the order they were
    /// raised, with globals interleaved where they landed.
    #[test]
    fn drains_in_the_order_raised() {
        let mut out = Intents::default();
        out.push(raise());
        out.extend([raise(), raise()]);
        out.push_dock(DockOp::CloseTab {
            tab: crate::core::document::TabRef::Preferences,
        });
        out.push(raise());

        let kinds: Vec<bool> = out
            .drain()
            .map(|item| matches!(item, Queued::Scoped(_)))
            .collect();
        assert_eq!(kinds, [true, true, true, false, true]);
        assert!(out.is_empty(), "draining empties the buffer");
    }
}
