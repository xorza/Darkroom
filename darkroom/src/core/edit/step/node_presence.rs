//! Whether a node is in the document at all.

use scenarium::{DetachedNode, NodeId};
use serde::{Deserialize, Serialize};

use crate::core::document::Document;
use crate::core::document::graph_view::ItemPlacement;
use crate::core::edit::step::change::{Change, Direction};
use crate::core::edit::step::reversible::Reversible;

/// Where a node's record lives in the document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum Home {
    Graph,
    Stubs,
}

/// Everything the document holds about one node: the record carrying every
/// edge that touched it and where it lives, where its body sits and how far
/// forward it paints, and whether it was selected. What a removal takes out,
/// and what an undo has to put back for the removal to have been reversible.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct NodeState {
    detached: DetachedNode,
    home: Home,
    /// Restored verbatim rather than recomputed, so an undone delete comes
    /// back where it was and at the depth it had — not frontmost.
    placement: ItemPlacement,
    /// Selection membership: removal prunes it, a restore re-adds it.
    selected: bool,
}

impl NodeState {
    /// Read `node_id`'s whole state out of `doc`, a graph node or a stub, or
    /// `None` when the document no longer holds it — the ordinary outcome of
    /// an intent that spans frames.
    ///
    /// One lookup answers both halves of the question: the snapshot *is* the
    /// existence check.
    pub(crate) fn capture(doc: &Document, node_id: NodeId) -> Option<Self> {
        let (detached, home) = match doc.graph.snapshot_node(node_id) {
            Some(detached) => (detached, Home::Graph),
            None => (doc.stubs.get(node_id)?.clone(), Home::Stubs),
        };
        Some(Self {
            placement: *doc
                .main_view
                .item_placements
                .get(&node_id)
                .expect("the view places every node and stub"),
            selected: doc.main_view.selected.contains(&node_id),
            detached,
            home,
        })
    }

    fn restore(&self, doc: &mut Document) {
        let node_id = self.detached.node_id();
        match self.home {
            Home::Graph => doc.graph.attach_node(self.detached.clone()),
            Home::Stubs => doc.stubs.insert(self.detached.clone()),
        }
        doc.main_view
            .item_placements
            .insert(node_id, self.placement);
        if self.selected {
            doc.main_view.selected.insert(node_id);
        }
    }
}

/// Whether one node is in the document, before and after.
///
/// Adding a node and removing one are the same edit in opposite directions,
/// so both intents lower to this one step rather than to a pair that would
/// have to keep their insert and remove halves in agreement. An insertion
/// carries `from: None`, a removal `to: None`, and a write either puts the
/// recorded state back or takes the node out.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct NodePresence {
    /// The node both halves are about. Read off the half that holds a state,
    /// so the anchor cannot disagree with the payload.
    node_id: NodeId,
    /// Exactly one half is `Some`: a presence step is an insertion or a
    /// removal, never a replacement of one node by another.
    state: Change<Option<NodeState>>,
}

impl NodePresence {
    /// Put a freshly authored node into the document; undo takes it back out.
    ///
    /// Unselected: adding a node leaves the selection alone, and a caller that
    /// wants the new node selected raises that as its own edit.
    pub(crate) const fn insertion(detached: DetachedNode, placement: ItemPlacement) -> Self {
        Self {
            node_id: detached.node_id(),
            state: Change {
                from: None,
                to: Some(NodeState {
                    detached,
                    home: Home::Graph,
                    placement,
                    selected: false,
                }),
            },
        }
    }

    /// Take `state`'s node out of the document; undo puts it back whole.
    pub(crate) const fn removal(state: NodeState) -> Self {
        Self {
            node_id: state.detached.node_id(),
            state: Change {
                from: Some(state),
                to: None,
            },
        }
    }

    /// Drop the node, and check that what came out is what the other half
    /// will put back.
    ///
    /// The two halves are this node's entire record, so a divergence means
    /// the document held wiring the history never captured — which an undo
    /// would silently discard. The check holds because a step is always
    /// reverted against exactly the document its own apply produced: entries
    /// replay in reverse, and so do the steps inside one.
    fn take_out(&self, doc: &mut Document, dir: Direction) {
        let recorded = self
            .state
            .half(dir.reversed())
            .as_ref()
            .expect("a presence step is an insertion or a removal, never two absences");
        let removed = match recorded.home {
            Home::Graph => doc.remove_node(self.node_id),
            Home::Stubs => doc.remove_stub(self.node_id),
        };
        assert_eq!(
            removed, recorded.detached,
            "removal diverged from the step that recorded it"
        );
    }
}

impl Reversible for NodePresence {
    fn write(&self, doc: &mut Document, dir: Direction) {
        match self.state.half(dir) {
            Some(state) => state.restore(doc),
            None => self.take_out(doc, dir),
        }
    }

    fn is_noop(&self) -> bool {
        self.state.unchanged()
    }

    fn dirties_document(&self) -> bool {
        true
    }

    /// A node arriving has no cached port offsets yet, so its wires have
    /// nothing to anchor to until it has recorded once — and a removal is
    /// true for its *revert*, which is exactly that arrival.
    fn invalidates_cached_geometry(&self) -> bool {
        true
    }

    fn changes_node_set(&self) -> bool {
        true
    }
}
