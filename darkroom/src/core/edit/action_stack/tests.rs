use std::collections::BTreeSet;
use std::sync::Arc;

use glam::Vec2;
use scenarium::NodeId;

use super::*;
use crate::core::document::Viewport;
use crate::core::document::harness::{self, DocFixture};
use crate::core::edit::graph_intent::GraphIntent;
use crate::core::edit::graph_intent::drag_start::DragStart;

/// A document and the history over it, edited only through the real
/// build/apply path — the pair every test below drives, so neither has to be
/// threaded through the helpers by hand.
#[derive(Debug)]
struct History {
    doc: Document,
    stack: ActionStack,
    /// The last gesture id minted.
    gestures: GestureId,
}

/// A latched drag: its gesture, and its members where they started.
#[derive(Debug)]
struct Drag {
    gesture: GestureId,
    members: Arc<[DragStart]>,
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
            gestures: GestureId::default(),
        }
    }

    /// The `i`th node of the sample, in its paint order.
    fn node(&self, i: usize) -> NodeId {
        harness::nth_in_paint_order(&self.doc.main_view, i)
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
        let mut steps: Vec<UndoStep> = intents.into_iter().map(|i| self.apply(i)).collect();
        self.stack.push(&mut steps, None);
        assert!(steps.is_empty(), "the stack takes the whole batch");
    }

    /// One frame of the held gesture `intent` names.
    fn frame(&mut self, intent: GraphIntent) {
        let gesture = intent.gesture();
        assert!(gesture.is_some(), "a frame names its gesture");
        let step = self.apply(intent);
        self.stack.push(&mut vec![step], gesture);
    }

    fn apply(&mut self, intent: GraphIntent) -> UndoStep {
        let step = intent
            .into_step(&self.doc)
            .unwrap()
            .expect("a test commits every intent");
        step.apply(&mut self.doc);
        step
    }

    /// A drag gesture of `members`, latched where they sit now.
    fn latch(&mut self, members: &[NodeId]) -> Drag {
        self.gestures = self.gestures.next();
        Drag {
            gesture: self.gestures,
            members: members
                .iter()
                .map(|&node| DragStart {
                    node,
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

/// The frames of one drag fold into one entry: one undo restores where the
/// drag started (the first `from`), and redo replays to its last `to`. Until
/// the drag is sealed nothing is packed, so a drag frame encodes nothing.
#[test]
fn the_frames_of_one_drag_are_one_entry() {
    let mut h = History::sample();
    let (a, b) = (h.node(0), h.node(1));
    let (a0, b0) = (h.pos(a), h.pos(b));

    let drag = h.latch(&[a, b]);
    let last = Vec2::new(25.0, 5.0);
    for offset in [Vec2::new(10.0, 0.0), last] {
        h.drag(&drag, offset);
    }
    assert_eq!((h.pos(a), h.pos(b)), (a0 + last, b0 + last));
    assert!(h.stack.actions.is_empty(), "an open drag packs nothing");

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
    let mut h = History::sample();
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
    let mut h = History::sample();
    let (a, b) = (h.node(0), h.node(1));
    let a0 = h.pos(a);
    h.select([b]);

    let drag = h.latch(&[a]);
    h.drag(&drag, Vec2::new(30.0, 30.0));
    h.drag(&drag, Vec2::ZERO);
    assert_eq!(h.pos(a), a0);

    assert!(h.undo(), "the selection is the one entry left");
    assert!(h.doc.main_view.selected.is_empty());
    assert!(!h.undo(), "the cancelled drag recorded nothing");
}

/// Another edit seals the open gesture: frames of the same drag after it
/// start a new entry, from where the edit left the document.
#[test]
fn another_edit_seals_the_open_gesture() {
    let mut h = History::sample();
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
    assert_eq!(h.doc.main_view.selected, BTreeSet::from([b]));
    assert!(h.undo());
    assert!(h.doc.main_view.selected.is_empty(), "the edit");
    assert!(h.undo());
    assert_eq!(h.pos(a), a0, "the frames before it");
    assert!(!h.undo());
}

/// A pan's frames are one entry, and a one-shot camera jump after it — a
/// toolbar fit, Ctrl+0 — is its own.
#[test]
fn a_camera_jump_does_not_fold_into_the_pan_before_it() {
    let mut h = History::sample();
    let start = h.doc.main_view.viewport;
    let at = |x: f32, zoom| Viewport {
        pan: Vec2::new(x, 0.0),
        zoom,
    };

    let pan = h.gesture();
    for x in [10.0, 20.0] {
        h.frame(GraphIntent::SetViewport {
            to: at(x, start.zoom),
            gesture: Some(pan),
        });
    }
    h.edit(GraphIntent::SetViewport {
        to: at(20.0, 2.0),
        gesture: None,
    });

    assert!(h.undo());
    assert_eq!(h.doc.main_view.viewport, at(20.0, start.zoom), "the jump");
    assert!(h.undo());
    assert_eq!(h.doc.main_view.viewport, start, "the whole pan");
    assert!(!h.undo());
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

/// The history keeps the newest entries that fit the byte budget, evicts
/// the oldest by advancing `head`, and reclaims the dead prefix only once it
/// outgrows the budget.
///
/// Every entry here is one selection toggle — `{} → {n}` or `{n} → {}`,
/// one empty set and one single-member set either way — so all have one size
/// `e`, and the stack must keep exactly `floor(256 / e)` of them. The model
/// below follows `head` and the physical length byte for byte.
#[test]
fn history_bounded_by_byte_budget() {
    const BUDGET: usize = 256;
    let mut h = History::bounded(BUDGET);
    let node = h.node(0);

    let toggle = |i: usize| -> BTreeSet<NodeId> {
        if i.is_multiple_of(2) {
            BTreeSet::from([node])
        } else {
            BTreeSet::new()
        }
    };
    h.select(toggle(0));
    let e = h.stack.entries[0].len();
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
        assert_eq!(h.stack.entries.len(), live, "entries after push {i}");
        assert_eq!(h.stack.head, head, "head after push {i}");
        assert_eq!(
            h.stack.actions.len(),
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
