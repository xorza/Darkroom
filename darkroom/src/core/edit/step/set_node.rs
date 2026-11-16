//! [`SetNode`]: a node added to the document or removed from it.

use scenarium::{Binding, DetachedGhost, DetachedNode, GraphId, InputPort, NodeId};
use serde::{Deserialize, Serialize};

use crate::core::document::Document;
use crate::core::document::graph_view::NodePlacement;
use crate::core::document::item_id::ItemId;
use crate::core::document::wire_key::{DataWire, WireKey};
use crate::core::edit::edit_facts::EditFacts;
use crate::core::edit::step::change::{Direction, EntryEdit};
use crate::core::edit::step::reversible::Reversible;
use crate::core::edit::step::set_ties::SetTies;

/// What the document holds of one node, as the record its detach leaves: a graph node with its
/// wiring, or a ghost node with the ghosts that touch it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
enum Held {
    Node(DetachedNode),
    Ghost(DetachedGhost),
}

impl Held {
    const fn node_id(&self) -> NodeId {
        match self {
            Self::Node(record) => record.node_id(),
            Self::Ghost(record) => record.node_id(),
        }
    }
}

/// Everything the document holds about one node: the node or ghost node with every edge that touched
/// it, its placement, and whether it was selected. What a removal takes out and an undo puts
/// back.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct NodeEntry {
    held: Held,
    /// Restored verbatim, so an undone delete comes back where it was and at its depth, not
    /// frontmost.
    placement: NodePlacement,
    /// Selection membership: removal prunes it, a restore re-adds it.
    selected: bool,
}

impl NodeEntry {
    /// Read `node_id`'s whole entry out of `graph` of `document`, a node or a ghost node, or `None`
    /// when the graph no longer holds it, the ordinary outcome of an intent that spans frames.
    pub(crate) fn capture(document: &Document, graph: GraphId, node_id: NodeId) -> Option<Self> {
        let held = document.graphs.graph(graph);
        let held = match held.snapshot_node(node_id) {
            Some(record) => Held::Node(record),
            None => Held::Ghost(held.snapshot_ghost_node(node_id)?),
        };
        let view = document.view(graph);
        Some(Self {
            placement: *view
                .node_placements
                .get(&node_id)
                .expect("the view places every node and ghost node"),
            selected: view.selected.contains(&ItemId::Node(node_id)),
            held,
        })
    }

    /// The wires the graph holds on this node: none for a ghost node, whose
    /// wiring is ghosts.
    pub(crate) fn wires(&self) -> impl Iterator<Item = WireKey> + '_ {
        let (bindings, subscriptions) = match &self.held {
            Held::Node(record) => (record.bindings(), record.subscriptions()),
            Held::Ghost(_) => (&[][..], &[][..]),
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

    /// The ghost wires that touch this node, a node's or a ghost node's, which go with it and
    /// take their routes.
    pub(crate) fn ghost_wires(&self) -> impl Iterator<Item = WireKey> + '_ {
        let wiring = match &self.held {
            Held::Node(record) => record.ghost_wiring(),
            Held::Ghost(record) => record.wiring(),
        };
        let data = wiring
            .wires()
            .map(|(input, output)| WireKey::Data(DataWire { output, input }));
        data.chain(
            wiring
                .subscriptions()
                .map(|(subscription, _)| WireKey::Event(subscription)),
        )
    }

    const fn node_id(&self) -> NodeId {
        self.held.node_id()
    }

    fn restore(&self, document: &mut Document, graph: GraphId) {
        let slot = document.slot_mut(graph);
        match &self.held {
            Held::Node(record) => slot.graph.attach_node(record.clone()),
            Held::Ghost(record) => slot.graph.attach_ghost_node(record.clone()),
        }
        let node_id = self.node_id();
        slot.view.node_placements.insert(node_id, self.placement);
        if self.selected {
            slot.view.selected.insert(ItemId::Node(node_id));
        }
    }
}

/// A node's whole entry coming into the document or leaving it.
///
/// Adding a node and removing one are the same edit in opposite directions, so both intents
/// lower to this one step: a write either puts the recorded entry back or takes the node out, as
/// [`EntryEdit`] and the direction say.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct SetNode {
    entry: NodeEntry,
    edit: EntryEdit,
    /// The routes of the wires a removal ends, and each tie that only they held. Empty for an
    /// insertion.
    ties: SetTies,
}

impl SetNode {
    /// Put a freshly authored node into the document, unselected: a caller that wants it
    /// selected raises that as its own edit.
    pub(crate) fn insertion(detached: DetachedNode, placement: NodePlacement) -> Self {
        Self {
            entry: NodeEntry {
                held: Held::Node(detached),
                placement,
                selected: false,
            },
            edit: EntryEdit::Insertion,
            ties: SetTies::default(),
        }
    }

    /// Put `record` in where `entry`'s node was: its placement, depth and selection, as Replace
    /// puts a node in place of the one it takes out.
    pub(crate) fn insertion_in_place(entry: &NodeEntry, record: DetachedNode) -> Self {
        Self {
            entry: NodeEntry {
                held: Held::Node(record),
                placement: entry.placement,
                selected: entry.selected,
            },
            edit: EntryEdit::Insertion,
            ties: SetTies::default(),
        }
    }

    /// Take `entry`'s node out of the document, with `ties`, the routes of its wires.
    pub(crate) const fn removal(entry: NodeEntry, ties: SetTies) -> Self {
        Self {
            entry,
            edit: EntryEdit::Removal,
            ties,
        }
    }

    /// The graph inputs a removal's node fed, which its removal leaves unbound;
    /// none for an insertion.
    pub(crate) fn fed_inputs(&self) -> impl Iterator<Item = InputPort> + '_ {
        let node_id = self.entry.node_id();
        self.entry
            .wires()
            .filter(move |_| self.edit == EntryEdit::Removal)
            .filter_map(move |wire| match wire {
                WireKey::Data(DataWire { output, input }) if output.node_id == node_id => {
                    Some(input)
                }
                WireKey::Data(_) | WireKey::Event(_) => None,
            })
    }

    /// Drop the node, and check that what came out is what the step will put back: a divergence
    /// means the document held wiring the history never captured, which an undo would silently
    /// discard. It holds because a step is reverted against the document its own apply produced.
    fn take_out(&self, document: &mut Document, graph: GraphId) {
        let node_id = self.entry.node_id();
        let removed = match self.entry.held {
            Held::Node(_) => Held::Node(document.remove_node(graph, node_id)),
            Held::Ghost(_) => Held::Ghost(document.remove_ghost_node(graph, node_id)),
        };
        debug_assert_eq!(
            removed, self.entry.held,
            "removal diverged from the step that recorded it"
        );
    }
}

impl Reversible for SetNode {
    fn write(&self, document: &mut Document, graph: GraphId, dir: Direction) {
        if self.edit.puts_in(dir) {
            self.entry.restore(document, graph);
        } else {
            self.take_out(document, graph);
        }
        self.ties.write(document, graph, dir);
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
