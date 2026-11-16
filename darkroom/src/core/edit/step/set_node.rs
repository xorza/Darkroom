//! [`SetNode`]: a node added to the document or removed from it.

use scenarium::{Binding, DetachedNode, GraphId, InputPort, NodeId, Stub};
use serde::{Deserialize, Serialize};

use crate::core::document::Document;
use crate::core::document::graph_view::NodePlacement;
use crate::core::document::item_id::ItemId;
use crate::core::document::wire_key::{DataWire, WireKey};
use crate::core::edit::edit_facts::EditFacts;
use crate::core::edit::step::change::{Direction, EntryEdit};
use crate::core::edit::step::reversible::Reversible;
use crate::core::edit::step::set_ties::SetTies;

/// What the document holds of one node: a graph node, as the record its detach leaves, or a
/// stub, which keeps its record and why the load took it out.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
enum Held {
    Node(DetachedNode),
    Stub(Stub),
}

impl Held {
    const fn record(&self) -> &DetachedNode {
        match self {
            Self::Node(record) | Self::Stub(Stub { record, .. }) => record,
        }
    }
}

/// Everything the document holds about one node: the node or stub with every edge that touched
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
    /// Read `node_id`'s whole entry out of `graph` of `document`, a node or a stub, or `None`
    /// when the graph no longer holds it, the ordinary outcome of an intent that spans frames.
    pub(crate) fn capture(document: &Document, graph: GraphId, node_id: NodeId) -> Option<Self> {
        let held = document.graphs.graph(graph);
        let held = match held.snapshot_node(node_id) {
            Some(record) => Held::Node(record),
            None => Held::Stub(held.stub(node_id)?.clone()),
        };
        let view = document.view(graph);
        Some(Self {
            placement: *view
                .node_placements
                .get(&node_id)
                .expect("the view places every node and stub"),
            selected: view.selected.contains(&ItemId::Node(node_id)),
            held,
        })
    }

    /// The wires the graph holds on this node: none for a stub, whose
    /// record keeps its wiring off the graph.
    pub(crate) fn wires(&self) -> impl Iterator<Item = WireKey> + '_ {
        let (bindings, subscriptions) = match &self.held {
            Held::Node(record) => (record.bindings(), record.subscriptions()),
            Held::Stub(_) => (&[][..], &[][..]),
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

    const fn node_id(&self) -> NodeId {
        self.held.record().node_id()
    }

    fn restore(&self, document: &mut Document, graph: GraphId) {
        let slot = document.slot_mut(graph);
        match &self.held {
            Held::Node(record) => slot.graph.attach_node(record.clone()),
            Held::Stub(stub) => slot.graph.insert_stub(stub.clone()),
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
            Held::Stub(_) => Held::Stub(document.remove_stub(graph, node_id)),
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
