use glam::Vec2;
use scenarium::NodeId;
use std::collections::BTreeSet;

use super::*;
use crate::core::document::Document;
use crate::core::document::harness::DocFixture;
use crate::core::edit::intent::apply::apply_step;
use crate::core::edit::intent::build::build_step;
use crate::core::edit::intent::types::GraphIntent;

/// A document and the history over it, edited only through the real
/// build/apply path — the pair every test below drives, so neither has to be
/// threaded through the helpers by hand.
#[derive(Debug)]
struct History {
    doc: Document,
    stack: ActionStack,
}

impl History {
    /// [`TestGraph::sample`]'s multi-node graph, on a budget nothing here can
    /// overflow.
    fn sample() -> Self {
        Self::bounded(1 << 20)
    }

    /// [`Self::sample`] on a stated byte budget — for the trimming case.
    fn bounded(max_bytes: usize) -> Self {
        Self {
            doc: DocFixture::sample().doc,
            stack: ActionStack::new(max_bytes),
        }
    }

    /// The `i`th node of the graph, in insertion order.
    fn node(&self, i: usize) -> NodeId {
        self.doc
            .graph
            .iter()
            .nth(i)
            .expect("the sample holds that many nodes")
            .id
    }

    fn pos(&self, node_id: NodeId) -> Vec2 {
        self.doc.main_view.item_placements[&node_id].pos
    }

    /// Push one graph edit, as a widget's single intent does.
    fn edit(&mut self, intent: GraphIntent) {
        self.batch([intent]);
    }

    /// Push several edits as one undo entry: each built against the live
    /// document and applied before the next is built, exactly as
    /// `drain_requests` does with a frame's worth.
    fn batch(&mut self, intents: impl IntoIterator<Item = GraphIntent>) {
        let steps: Vec<UndoStep> = intents
            .into_iter()
            .map(|intent| {
                let step = build_step(intent, &self.doc).unwrap();
                apply_step(&step, &mut self.doc);
                step
            })
            .collect();
        self.stack.push_current(&steps);
    }

    /// One frame of a drag: `grabbed` names the node under the pointer, which
    /// is what the gesture key coalesces on.
    fn drag(&mut self, grabbed: NodeId, moves: Vec<(NodeId, Vec2)>) {
        self.edit(GraphIntent::MoveSelection { grabbed, moves });
    }

    fn select(&mut self, to: impl IntoIterator<Item = NodeId>) {
        self.edit(GraphIntent::SetSelection {
            to: to.into_iter().collect(),
        });
    }

    /// Take back one entry. The per-step callback is the editor's business —
    /// nothing here watches it.
    fn undo(&mut self) -> bool {
        self.stack.undo(&mut self.doc, &mut |_| {})
    }

    fn redo(&mut self) -> bool {
        self.stack.redo(&mut self.doc, &mut |_| {})
    }
}

#[test]
fn consecutive_moves_coalesce_keeping_first_from() {
    let mut h = History::sample();
    let key = h.node(0);
    let start = h.pos(key);

    h.drag(key, vec![(key, Vec2::new(10.0, 10.0))]);
    h.drag(key, vec![(key, Vec2::new(20.0, 20.0))]);

    // Both moves of the same node collapsed into one entry: a single
    // undo restores the *original* position (the first `from`)...
    assert!(h.undo());
    assert_eq!(h.pos(key), start, "one undo reverts the whole drag");
    assert!(!h.undo(), "the drag collapsed to exactly one entry");
    // ...and redo replays to the last `to`.
    assert!(h.redo());
    assert_eq!(h.pos(key), Vec2::new(20.0, 20.0));
}

#[test]
fn moves_of_different_nodes_do_not_coalesce() {
    let mut h = History::sample();
    let (a, b) = (h.node(0), h.node(1));

    h.drag(a, vec![(a, Vec2::new(5.0, 5.0))]);
    h.drag(b, vec![(b, Vec2::new(6.0, 6.0))]);

    // Different grabbed nodes ⇒ different `SelectionDrag` keys ⇒ two entries.
    assert!(h.undo());
    assert!(
        h.undo(),
        "moves of distinct nodes stay separate undo entries"
    );
}

#[test]
fn group_drag_moves_all_and_undoes_as_one() {
    let mut h = History::sample();
    let (ka, kb) = (h.node(0), h.node(1));
    let (a0, b0) = (h.pos(ka), h.pos(kb));

    // Two frames of a group drag (grabbed = a), each frame moving both nodes
    // by the running offset. Same grabbed ⇒ one coalesced entry.
    let last = Vec2::new(25.0, 5.0);
    for off in [Vec2::new(10.0, 0.0), last] {
        h.drag(ka, vec![(ka, a0 + off), (kb, b0 + off)]);
    }
    assert_eq!(h.pos(ka), a0 + last, "both ended at origin + last offset");
    assert_eq!(h.pos(kb), b0 + last);

    // One undo restores both to their pre-drag positions (first `from`).
    assert!(h.undo());
    assert_eq!(h.pos(ka), a0);
    assert_eq!(h.pos(kb), b0);
    assert!(!h.undo(), "the group drag collapsed to exactly one entry");
}

#[test]
fn deleting_selection_restores_nodes_and_edge_in_one_undo() {
    use scenarium::{Binding, InputPort};

    let mut h = History::sample();
    let (a, b) = (h.node(0), h.node(1));
    // Edge a -> b, then select both for deletion.
    h.doc
        .graph
        .set_input_binding(InputPort::new(b, 0), Binding::bind(a, 0));
    h.doc.main_view.selected = [a, b].into_iter().collect();

    // The a->b edge is captured by a's step (before a is removed), so a
    // single undo can restore it once both nodes are back.
    h.batch([a, b].map(|node_id| GraphIntent::RemoveNode { node_id }));
    assert!(h.doc.graph.find(a).is_none());
    assert!(h.doc.graph.find(b).is_none());

    assert!(h.undo());
    assert!(h.doc.graph.find(a).is_some());
    assert!(h.doc.graph.find(b).is_some());
    match h.doc.graph.bindings.get(&InputPort::new(b, 0)) {
        Some(Binding::Bind(src)) => assert_eq!((src.node_id, src.port_idx), (a, 0)),
        other => panic!("expected restored a->b edge, got {other:?}"),
    }
    assert!(!h.undo(), "the whole delete collapsed to one undo entry");
}

#[test]
fn new_edit_discards_the_redo_tail() {
    let mut h = History::sample();
    let node = h.node(0);

    h.select([node]); // A: {} -> {node}
    h.select([]); // B: {node} -> {}

    // Undo B → selection back to {node}, B now redoable.
    assert!(h.undo());
    // A fresh edit while a redo is pending must discard it.
    h.select([]); // C: {node} -> {}
    assert!(!h.redo(), "a new edit invalidates the redoable tail");
}

#[test]
fn history_bounded_by_byte_budget() {
    // Tiny budget so a handful of small entries overflow it.
    let mut h = History::bounded(256);
    let node = h.node(0);

    // Many distinct, non-coalescing selection edits (toggle one node
    // in/out — `from != to` each time, gesture key `None`).
    for i in 0..200 {
        let to: BTreeSet<NodeId> = if i % 2 == 0 {
            [node].into_iter().collect()
        } else {
            BTreeSet::new()
        };
        h.select(to);
        // The *live* region stays within budget (entries are far
        // smaller than 256 B, so no single-entry overflow)...
        let live = h.stack.actions.len() - h.stack.head;
        assert!(
            live <= h.stack.max_bytes,
            "live {live} exceeded budget {} after push {i}",
            h.stack.max_bytes,
        );
        // ...and the dead-prefix reclaim keeps the physical buffer
        // bounded (lazy compaction fires at head > budget).
        assert!(
            h.stack.actions.len() <= 2 * h.stack.max_bytes,
            "physical buffer {} exceeded 2× budget after push {i}",
            h.stack.actions.len(),
        );
    }

    // Old entries were dropped (not all 200 kept) and the newest is
    // still undoable.
    assert!(
        h.stack.entries.len() < 200,
        "oldest entries should have been trimmed"
    );
    assert!(h.undo(), "the most recent edit stays undoable");
}
