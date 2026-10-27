use std::collections::BTreeSet;
use std::sync::Arc;

use glam::Vec2;
use scenarium::{Library, NodeId};

use super::*;
use crate::core::document::internals::{self, DocumentFixture};
use crate::core::document::item_id::ItemId;
use crate::core::edit::graph_intent::GraphIntent;
use crate::core::edit::graph_intent::drag_start::DragStart;

/// A document and the history over it, edited only through the real
/// build/apply path — the pair every test below drives, so neither has to be
/// threaded through the helpers by hand.
#[derive(Debug)]
struct Harness {
    document: Document,
    library: Library,
    history: UndoHistory,
    /// The last gesture id minted.
    gestures: GestureId,
}

/// A latched drag: its gesture, and its members where they started.
#[derive(Debug)]
struct Drag {
    gesture: GestureId,
    members: Arc<[DragStart]>,
}

impl Harness {
    /// [`TestGraph::sample`]'s multi-node graph, on a budget nothing here can
    /// overflow.
    fn sample() -> Self {
        Self::bounded(1 << 20)
    }

    /// [`Self::sample`] on a stated byte budget — for the trimming case.
    fn bounded(max_bytes: usize) -> Self {
        let DocumentFixture { document, library } = DocumentFixture::sample();
        Self {
            document,
            library,
            history: UndoHistory::new(max_bytes),
            gestures: GestureId::default(),
        }
    }

    /// The `i`th node of the sample, in its paint order.
    fn node(&self, i: usize) -> NodeId {
        internals::nth_in_paint_order(&self.document.main_view, i)
    }

    fn pos(&self, node_id: NodeId) -> Vec2 {
        self.document.main_view.node_placements[&node_id].pos
    }

    /// Push one graph edit, as a widget's single intent does.
    fn edit(&mut self, intent: GraphIntent) {
        self.batch([intent]);
    }

    /// Push several edits as one undo entry: each built against the live
    /// document and applied before the next is built, exactly as
    /// `drain_requests` does with a frame's worth.
    fn batch(&mut self, intents: impl IntoIterator<Item = GraphIntent>) {
        let mut steps = Vec::new();
        for intent in intents {
            self.apply(intent, &mut steps);
        }
        self.history.push(&mut steps, None);
        assert!(steps.is_empty(), "the history takes the whole batch");
    }

    /// One frame of the held gesture `intent` names.
    fn frame(&mut self, intent: GraphIntent) {
        let gesture = intent.gesture();
        assert!(gesture.is_some(), "a frame names its gesture");
        let mut steps = Vec::new();
        self.apply(intent, &mut steps);
        assert_eq!(steps.len(), 1, "a frame commits one step");
        self.history.push(&mut steps, gesture);
    }

    /// Commit `intent` onto `steps`, as `drain_requests` does.
    fn apply(&mut self, intent: GraphIntent, steps: &mut Vec<UndoStep>) {
        let committed = steps.len();
        intent
            .commit(&mut self.document, &self.library, steps)
            .expect("the intent is well formed");
        assert!(steps.len() > committed, "a test commits every intent");
    }

    /// A drag gesture of `members`, latched where they sit now.
    fn latch(&mut self, members: &[NodeId]) -> Drag {
        Drag {
            gesture: self.gesture(),
            members: members
                .iter()
                .map(|&node| DragStart {
                    item: ItemId::Node(node),
                    pos: self.pos(node),
                })
                .collect(),
        }
    }

    /// One frame of `drag`, every member at its start plus `offset`.
    fn drag(&mut self, drag: &Drag, offset: Vec2) {
        self.frame(GraphIntent::MoveSelection {
            gesture: drag.gesture,
            members: Arc::clone(&drag.members),
            offset,
        });
    }

    /// A fresh gesture id, as a press mints one.
    fn gesture(&mut self) -> GestureId {
        self.gestures = self.gestures.next();
        self.gestures
    }

    fn select(&mut self, to: impl IntoIterator<Item = NodeId>) {
        self.edit(GraphIntent::SetSelection {
            to: to.into_iter().map(ItemId::Node).collect(),
        });
    }

    /// Take back one entry. The per-step callback is the editor's business —
    /// nothing here watches it.
    fn undo(&mut self) -> bool {
        self.history.undo(&mut self.document, &mut |_| {})
    }

    fn redo(&mut self) -> bool {
        self.history.redo(&mut self.document, &mut |_| {})
    }
}

/// The frames of one drag fold into one entry: one undo restores where the
/// drag started (the first `from`), and redo replays to its last `to`. Until
/// the drag is sealed nothing is packed, so a drag frame encodes nothing.
#[test]
fn the_frames_of_one_drag_are_one_entry() {
    let mut h = Harness::sample();
    let (a, b) = (h.node(0), h.node(1));
    let (a0, b0) = (h.pos(a), h.pos(b));

    let drag = h.latch(&[a, b]);
    let last = Vec2::new(25.0, 5.0);
    for offset in [Vec2::new(10.0, 0.0), last] {
        h.drag(&drag, offset);
    }
    assert_eq!((h.pos(a), h.pos(b)), (a0 + last, b0 + last));
    assert!(h.history.bytes.is_empty(), "an open drag packs nothing");

    assert!(h.undo());
    assert_eq!((h.pos(a), h.pos(b)), (a0, b0), "one undo reverts the drag");
    assert!(!h.undo(), "the drag is exactly one entry");
    assert!(h.redo());
    assert_eq!((h.pos(a), h.pos(b)), (a0 + last, b0 + last));
}

/// Two drags of the same node are two gestures, so two entries, even with no
/// edit between them.
#[test]
fn two_drags_of_one_node_stay_two_entries() {
    let mut h = Harness::sample();
    let a = h.node(0);
    let a0 = h.pos(a);

    let first = h.latch(&[a]);
    h.drag(&first, Vec2::new(5.0, 0.0));
    let second = h.latch(&[a]);
    h.drag(&second, Vec2::new(0.0, 7.0));
    assert_eq!(h.pos(a), a0 + Vec2::new(5.0, 7.0));

    assert!(h.undo());
    assert_eq!(h.pos(a), a0 + Vec2::new(5.0, 0.0), "the second drag undone");
    assert!(h.undo());
    assert_eq!(h.pos(a), a0, "the first drag undone");
    assert!(!h.undo());
}

/// A drag that ends where it started — an Esc cancel puts every member back —
/// records nothing.
#[test]
fn a_drag_back_to_its_start_records_nothing() {
    let mut h = Harness::sample();
    let (a, b) = (h.node(0), h.node(1));
    let a0 = h.pos(a);
    h.select([b]);

    let drag = h.latch(&[a]);
    h.drag(&drag, Vec2::new(30.0, 30.0));
    h.drag(&drag, Vec2::ZERO);
    assert_eq!(h.pos(a), a0);

    assert!(h.undo(), "the selection is the one entry left");
    assert!(h.document.main_view.selected.is_empty());
    assert!(!h.undo(), "the cancelled drag recorded nothing");
}

/// Another edit seals the open gesture: frames of the same drag after it
/// start a new entry, from where the edit left the document.
#[test]
fn another_edit_seals_the_open_gesture() {
    let mut h = Harness::sample();
    let (a, b) = (h.node(0), h.node(1));
    let a0 = h.pos(a);

    let drag = h.latch(&[a]);
    h.drag(&drag, Vec2::new(4.0, 0.0));
    h.select([b]);
    h.drag(&drag, Vec2::new(9.0, 0.0));

    assert!(h.undo());
    assert_eq!(
        h.pos(a),
        a0 + Vec2::new(4.0, 0.0),
        "the frames after the edit"
    );
    assert_eq!(
        h.document.main_view.selected,
        BTreeSet::from([ItemId::Node(b)])
    );
    assert!(h.undo());
    assert!(h.document.main_view.selected.is_empty(), "the edit");
    assert!(h.undo());
    assert_eq!(h.pos(a), a0, "the frames before it");
    assert!(!h.undo());
}

#[test]
fn deleting_selection_restores_nodes_and_edge_in_one_undo() {
    use scenarium::{Binding, InputPort};

    let mut h = Harness::sample();
    let named = |h: &Harness, name: &str| {
        h.document
            .graph
            .iter()
            .find(|node| node.name == name)
            .expect("the sample holds the node")
            .node_id
    };
    let (a, b) = (named(&h, "get_b"), named(&h, "sum"));
    // Edge a -> b, then select both for deletion.
    h.document
        .graph
        .set_input_binding(InputPort::new(b, 0), Binding::wire(a, 0));
    h.document.main_view.selected = [a, b].map(ItemId::Node).into_iter().collect();

    // The a->b edge is captured by a's step (before a is removed), so a
    // single undo can restore it once both nodes are back.
    h.batch([a, b].map(|node_id| GraphIntent::RemoveNode { node_id }));
    assert!(h.document.graph.find(a).is_none());
    assert!(h.document.graph.find(b).is_none());

    assert!(h.undo());
    assert!(h.document.graph.find(a).is_some());
    assert!(h.document.graph.find(b).is_some());
    match h.document.graph.bindings.get(&InputPort::new(b, 0)) {
        Some(Binding::Wire(src)) => assert_eq!((src.node_id, src.port_idx), (a, 0)),
        other => panic!("expected restored a->b edge, got {other:?}"),
    }
    assert!(!h.undo(), "the whole delete collapsed to one undo entry");
}

#[test]
fn new_edit_discards_the_redo_tail() {
    let mut h = Harness::sample();
    let node = h.node(0);

    h.select([node]); // A: {} -> {node}
    h.select([]); // B: {node} -> {}

    // Undo B → selection back to {node}, B now redoable.
    assert!(h.undo());
    // A fresh edit while a redo is pending must discard it.
    h.select([]); // C: {node} -> {}
    assert!(!h.redo(), "a new edit invalidates the redoable tail");
}

/// The history keeps the newest entries that fit the byte budget, evicts
/// the oldest by advancing `head`, and reclaims the dead prefix only once it
/// outgrows the budget.
///
/// Every entry here is one selection toggle — `{} → {n}` or `{n} → {}`,
/// one empty set and one single-member set either way — so all have one size
/// `e`, and the history must keep exactly `floor(256 / e)` of them. The model
/// below follows `head` and the physical length byte for byte.
#[test]
fn history_bounded_by_byte_budget() {
    const BUDGET: usize = 256;
    let mut h = Harness::bounded(BUDGET);
    let node = h.node(0);

    let toggle = |i: usize| -> BTreeSet<NodeId> {
        if i.is_multiple_of(2) {
            BTreeSet::from([node])
        } else {
            BTreeSet::new()
        }
    };
    h.select(toggle(0));
    let e = h.history.entries[0].len();
    let kept = BUDGET / e;
    assert!(kept >= 2, "the budget holds several entries of {e} bytes");

    let (mut live, mut head) = (1, 0);
    for i in 1..200 {
        h.select(toggle(i));
        live += 1;
        if live * e > BUDGET {
            live -= 1;
            head += e;
            if head > BUDGET {
                head = 0;
            }
        }
        assert_eq!(h.history.entries.len(), live, "entries after push {i}");
        assert_eq!(h.history.head, head, "head after push {i}");
        assert_eq!(
            h.history.bytes.len(),
            head + live * e,
            "bytes after push {i}"
        );
    }
    assert_eq!(live, kept);

    for _ in 0..kept {
        assert!(h.undo());
    }
    assert!(!h.undo(), "exactly the newest {kept} entries were kept");
}

/// A full rewind then a fresh edit reclaims the whole buffer, dead prefix and
/// all: the history is the one new entry. A drag frame drops the redo tail as
/// any edit does, and a drag back to its start is nothing to undo while it is
/// still held.
#[test]
fn a_rewind_then_an_edit_starts_the_buffer_over() {
    let mut h = Harness::sample();
    let (a, b) = (h.node(0), h.node(1));
    h.select([a]);
    h.select([b]);
    assert!(h.undo() && h.undo());
    h.select([a]);
    assert_eq!(h.history.entries.len(), 1);
    assert_eq!(h.history.head, 0);
    assert_eq!(h.history.bytes.len(), h.history.entries[0].len());

    assert!(h.undo());
    let drag = h.latch(&[a]);
    h.drag(&drag, Vec2::new(3.0, 0.0));
    assert!(!h.redo(), "the drag frame dropped the selection's redo");

    let mut still = Harness::sample();
    let node = still.node(0);
    let held = still.latch(&[node]);
    still.drag(&held, Vec2::new(3.0, 0.0));
    still.drag(&held, Vec2::ZERO);
    assert!(
        !still.history.can_undo(),
        "a held drag back where it started"
    );
}

/// One entry larger than the whole budget is kept on its own, the older ones
/// dropped: the edit just made is always undoable.
#[test]
fn an_entry_over_the_budget_survives_alone() {
    let mut h = Harness::bounded(1);
    let (a, b) = (h.node(0), h.node(1));
    h.select([a]);
    h.select([a, b]);
    assert_eq!(h.history.entries.len(), 1, "the older one dropped");
    assert!(h.undo());
    assert_eq!(
        h.document.main_view.selected,
        BTreeSet::from([ItemId::Node(a)])
    );
    assert!(!h.undo());
}

/// A withdraw takes the held gesture's whole entry back, newest step first,
/// and forgets it; a withdraw of another gesture, or of one an edit sealed,
/// touches nothing.
#[test]
fn a_withdraw_takes_the_open_gesture_back() {
    let mut h = Harness::sample();
    let a = h.node(0);
    let before = h.document.clone();
    let drag = h.latch(&[a]);
    h.drag(&drag, Vec2::new(7.0, 0.0));
    let other = h.gesture();
    assert!(!h.history.withdraw(other, &mut h.document, &mut |_| {}));
    assert!(
        h.history
            .withdraw(drag.gesture, &mut h.document, &mut |_| {})
    );
    assert_eq!(h.document, before);
    assert!(!h.history.can_undo(), "nothing is left");

    let sealed = h.latch(&[a]);
    h.drag(&sealed, Vec2::new(7.0, 0.0));
    h.select([a]);
    let moved = h.document.clone();
    assert!(
        !h.history
            .withdraw(sealed.gesture, &mut h.document, &mut |_| {})
    );
    assert_eq!(h.document, moved);
}
