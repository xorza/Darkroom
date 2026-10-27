//! [`SetNode`]: a node added to the document or removed from it.

use scenarium::{Binding, DetachedNode, InputPort, NodeId};
use serde::{Deserialize, Serialize};

use crate::core::document::Document;
use crate::core::document::graph_view::NodePlacement;
use crate::core::document::item_id::ItemId;
use crate::core::document::wire_key::{DataWire, WireKey};
use crate::core::edit::edit_facts::EditFacts;
use crate::core::edit::step::change::Direction;
use crate::core::edit::step::reversible::Reversible;
use crate::core::edit::step::set_ties::SetTies;

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
pub(crate) struct NodeEntry {
    detached: DetachedNode,
    home: Home,
    /// Restored verbatim rather than recomputed, so an undone delete comes
    /// back where it was and at the depth it had — not frontmost.
    placement: NodePlacement,
    /// Selection membership: removal prunes it, a restore re-adds it.
    selected: bool,
}

impl NodeEntry {
    /// Read `node_id`'s whole entry out of `document`, a graph node or a stub, or
    /// `None` when the document no longer holds it — the ordinary outcome of
    /// an intent that spans frames.
    ///
    /// One lookup answers both halves of the question: the snapshot *is* the
    /// existence check.
    pub(crate) fn capture(document: &Document, node_id: NodeId) -> Option<Self> {
        let (detached, home) = match document.graph.snapshot_node(node_id) {
            Some(detached) => (detached, Home::Graph),
            None => (document.stubs.get(node_id)?.clone(), Home::Stubs),
        };
        Some(Self {
            placement: *document
                .main_view
                .node_placements
                .get(&node_id)
                .expect("the view places every node and stub"),
            selected: document.main_view.selected.contains(&ItemId::Node(node_id)),
            detached,
            home,
        })
    }

    /// The wires the graph holds on this node: none for a stub, whose
    /// record keeps its wiring off the graph.
    pub(crate) fn wires(&self) -> impl Iterator<Item = WireKey> + '_ {
        let (bindings, subscriptions) = match self.home {
            Home::Graph => (self.detached.bindings(), self.detached.subscriptions()),
            Home::Stubs => (&[][..], &[][..]),
        };
        let data = bindings.iter().filter_map(|entry| match entry.binding {
            Binding::Wire(output) => Some(WireKey::Data(DataWire {
                output,
                input: entry.port,
            })),
            Binding::Const(_) => None,
        });
        data.chain(subscriptions.iter().copied().map(WireKey::Event))
    }

    fn restore(&self, document: &mut Document) {
        let node_id = self.detached.node_id();
        match self.home {
            Home::Graph => document.graph.attach_node(self.detached.clone()),
            Home::Stubs => document.stubs.insert(self.detached.clone()),
        }
        document
            .main_view
            .node_placements
            .insert(node_id, self.placement);
        if self.selected {
            document.main_view.selected.insert(ItemId::Node(node_id));
        }
    }
}

/// A node's whole entry coming into the document or leaving it.
///
/// Adding a node and removing one are the same edit in opposite directions,
/// so both intents lower to this one step rather than to a pair that would
/// have to keep their insert and remove halves in agreement: a write either
/// puts the recorded entry back or takes the node out, as [`NodeEdit`] and
/// the direction say.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct SetNode {
    entry: NodeEntry,
    edit: NodeEdit,
    /// The routes of the wires a removal ends, and each tie that only they
    /// held. Empty for an insertion, which ends no wire.
    ties: SetTies,
}

/// Which way a [`SetNode`] moves its entry when it applies.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum NodeEdit {
    Insertion,
    Removal,
}

impl SetNode {
    /// Put a freshly authored node into the document; undo takes it back out.
    ///
    /// Unselected: adding a node leaves the selection alone, and a caller that
    /// wants the new node selected raises that as its own edit.
    pub(crate) fn insertion(detached: DetachedNode, placement: NodePlacement) -> Self {
        Self {
            entry: NodeEntry {
                detached,
                home: Home::Graph,
                placement,
                selected: false,
            },
            edit: NodeEdit::Insertion,
            ties: SetTies::default(),
        }
    }

    /// Take `entry`'s node out of the document, with `ties`, the routes of
    /// its wires; undo puts it back whole.
    pub(crate) const fn removal(entry: NodeEntry, ties: SetTies) -> Self {
        Self {
            entry,
            edit: NodeEdit::Removal,
            ties,
        }
    }

    /// The graph inputs a removal's node fed, which its removal leaves unbound;
    /// none for an insertion.
    pub(crate) fn fed_inputs(&self) -> impl Iterator<Item = InputPort> + '_ {
        let node_id = self.entry.detached.node_id();
        self.entry
            .wires()
            .filter(move |_| self.edit == NodeEdit::Removal)
            .filter_map(move |wire| match wire {
                WireKey::Data(DataWire { output, input }) if output.node_id == node_id => {
                    Some(input)
                }
                WireKey::Data(_) | WireKey::Event(_) => None,
            })
    }

    /// Drop the node, and check that what came out is what the step will put
    /// back.
    ///
    /// The entry is this node's whole record, so a divergence means the
    /// document held wiring the history never captured — which an undo would
    /// silently discard. The check holds because a step is always reverted
    /// against exactly the document its own apply produced: entries replay in
    /// reverse, and so do the steps inside one.
    fn take_out(&self, document: &mut Document) {
        let node_id = self.entry.detached.node_id();
        let removed = match self.entry.home {
            Home::Graph => document.remove_node(node_id),
            Home::Stubs => document.remove_stub(node_id),
        };
        debug_assert_eq!(
            removed, self.entry.detached,
            "removal diverged from the step that recorded it"
        );
    }
}

impl Reversible for SetNode {
    fn write(&self, document: &mut Document, dir: Direction) {
        match (self.edit, dir) {
            (NodeEdit::Insertion, Direction::Forward)
            | (NodeEdit::Removal, Direction::Backward) => {
                self.entry.restore(document);
            }
            (NodeEdit::Insertion, Direction::Backward)
            | (NodeEdit::Removal, Direction::Forward) => {
                self.take_out(document);
            }
        }
        self.ties.write(document, dir);
    }

    /// A node always comes or goes.
    fn is_noop(&self) -> bool {
        false
    }

    fn dirties_document(&self) -> bool {
        true
    }

    fn facts(&self) -> EditFacts {
        EditFacts {
            node_set: true,
            ..EditFacts::NONE
        }
    }
}
