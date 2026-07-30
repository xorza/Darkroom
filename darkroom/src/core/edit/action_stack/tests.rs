use scenarium::NodeId;
use std::collections::BTreeSet;

use super::*;
use crate::core::document::Document;
use crate::core::edit::intent::apply::apply_step;
use crate::core::edit::intent::build::build_step;
use crate::core::edit::intent::types::Intent;
use scenarium::testing::test_graph;

/// Push one graph edit through the real build/apply path, as `drain_intents`
/// does — the shape every coalescing test below repeats.
fn push_edit(stack: &mut ActionStack, doc: &mut Document, intent: Intent) {
    let step = build_step(intent, doc).unwrap();
    apply_step(&step, doc);
    stack.push_current(&[step]);
}

#[test]
fn consecutive_moves_coalesce_keeping_first_from() {
    use glam::Vec2;

    let mut doc: Document = test_graph().into();
    let key = doc.graph.iter().next().unwrap().id;
    let start = *doc.main_view.item_placements.get(&key).unwrap();
    let mut stack = ActionStack::new(1 << 20);

    let drag_to = |stack: &mut ActionStack, doc: &mut Document, to: Vec2| {
        push_edit(
            stack,
            doc,
            Intent::MoveSelection {
                grabbed: key,
                moves: vec![(key, to)],
            },
        );
    };
    drag_to(&mut stack, &mut doc, Vec2::new(10.0, 10.0));
    drag_to(&mut stack, &mut doc, Vec2::new(20.0, 20.0));

    // Both moves of the same node collapsed into one entry: a single
    // undo restores the *original* position (the first `from`)...
    assert!(stack.undo(&mut doc, &mut |_| {}));
    assert_eq!(
        doc.main_view.item_placements[&key], start,
        "one undo reverts the whole drag"
    );
    assert!(
        !stack.undo(&mut doc, &mut |_| {}),
        "the drag collapsed to exactly one entry"
    );
    // ...and redo replays to the last `to`.
    assert!(stack.redo(&mut doc, &mut |_| {}));
    assert_eq!(doc.main_view.item_placements[&key], Vec2::new(20.0, 20.0));
}

#[test]
fn moves_of_different_nodes_do_not_coalesce() {
    use glam::Vec2;

    let mut doc: Document = test_graph().into();
    let a = doc.graph.iter().next().unwrap().id;
    let b = doc.graph.iter().nth(1).unwrap().id;
    let mut stack = ActionStack::new(1 << 20);

    for (key, to) in [(a, Vec2::new(5.0, 5.0)), (b, Vec2::new(6.0, 6.0))] {
        push_edit(
            &mut stack,
            &mut doc,
            Intent::MoveSelection {
                grabbed: key,
                moves: vec![(key, to)],
            },
        );
    }
    // Different grabbed nodes ⇒ different `SelectionDrag` keys ⇒ two entries.
    assert!(stack.undo(&mut doc, &mut |_| {}));
    assert!(
        stack.undo(&mut doc, &mut |_| {}),
        "moves of distinct nodes stay separate undo entries"
    );
}

#[test]
fn group_drag_moves_all_and_undoes_as_one() {
    use glam::Vec2;

    let mut doc: Document = test_graph().into();
    let ka = doc.graph.iter().next().unwrap().id;
    let kb = doc.graph.iter().nth(1).unwrap().id;
    let a0 = doc.main_view.item_placements[&ka];
    let b0 = doc.main_view.item_placements[&kb];
    let mut stack = ActionStack::new(1 << 20);

    // Two frames of a group drag (grabbed = a), each frame moving both nodes
    // by the running offset. Same grabbed ⇒ one coalesced entry.
    let drag = |stack: &mut ActionStack, doc: &mut Document, off: Vec2| {
        push_edit(
            stack,
            doc,
            Intent::MoveSelection {
                grabbed: ka,
                moves: vec![(ka, a0 + off), (kb, b0 + off)],
            },
        );
    };
    drag(&mut stack, &mut doc, Vec2::new(10.0, 0.0));
    drag(&mut stack, &mut doc, Vec2::new(25.0, 5.0));

    // Both ended at origin + last offset.
    let item_pos = |doc: &Document, key: &NodeId| -> Vec2 { doc.main_view.item_placements[key] };
    assert_eq!(item_pos(&doc, &ka), a0 + Vec2::new(25.0, 5.0));
    assert_eq!(item_pos(&doc, &kb), b0 + Vec2::new(25.0, 5.0));

    // One undo restores both to their pre-drag positions (first `from`).
    assert!(stack.undo(&mut doc, &mut |_| {}));
    assert_eq!(item_pos(&doc, &ka), a0);
    assert_eq!(item_pos(&doc, &kb), b0);
    assert!(
        !stack.undo(&mut doc, &mut |_| {}),
        "the group drag collapsed to exactly one entry"
    );
}

#[test]
fn deleting_selection_restores_nodes_and_edge_in_one_undo() {
    use scenarium::{Binding, InputPort};

    let mut doc: Document = test_graph().into();
    let a = doc.graph.iter().next().unwrap().id;
    let b = doc.graph.iter().nth(1).unwrap().id;
    // Edge a -> b, then select both for deletion.
    doc.graph
        .set_input_binding(InputPort::new(b, 0), Binding::bind(a, 0));
    doc.main_view.selected = [a, b].into_iter().collect();

    // Mirror `drain_intents`: build each `RemoveNode` against the live
    // doc, apply immediately, collect into one batch entry. The a->b
    // edge is captured by a's step (before a is removed), so a single
    // undo can restore it once both nodes are back.
    let mut stack = ActionStack::new(1 << 20);
    let mut batch = Vec::new();
    for node_id in [a, b] {
        let step = build_step(Intent::RemoveNode { node_id }, &doc).unwrap();
        apply_step(&step, &mut doc);
        batch.push(step);
    }
    stack.push_current(&batch);

    assert!(doc.graph.find(a).is_none());
    assert!(doc.graph.find(b).is_none());

    assert!(stack.undo(&mut doc, &mut |_| {}));
    assert!(doc.graph.find(a).is_some());
    assert!(doc.graph.find(b).is_some());
    match doc.graph.bindings.get(&InputPort::new(b, 0)) {
        Some(Binding::Bind(src)) => assert_eq!((src.node_id, src.port_idx), (a, 0)),
        other => panic!("expected restored a->b edge, got {other:?}"),
    }
    assert!(
        !stack.undo(&mut doc, &mut |_| {}),
        "the whole delete collapsed to one undo entry"
    );
}

#[test]
fn new_edit_discards_the_redo_tail() {
    let mut doc: Document = test_graph().into();
    let node = doc.graph.iter().next().unwrap().id;
    let mut stack = ActionStack::new(1 << 20);

    let one: BTreeSet<_> = [node].into_iter().collect();
    push_edit(&mut stack, &mut doc, Intent::SetSelection { to: one }); // A: {} -> {node}
    push_edit(
        &mut stack,
        &mut doc,
        Intent::SetSelection {
            to: BTreeSet::new(),
        },
    ); // B: {node} -> {}

    // Undo B → selection back to {node}, B now redoable.
    assert!(stack.undo(&mut doc, &mut |_| {}));
    // A fresh edit while a redo is pending must discard it.
    push_edit(
        &mut stack,
        &mut doc,
        Intent::SetSelection {
            to: BTreeSet::new(),
        },
    ); // C: {node} -> {}
    assert!(
        !stack.redo(&mut doc, &mut |_| {}),
        "a new edit invalidates the redoable tail"
    );
}

#[test]
fn history_bounded_by_byte_budget() {
    let mut doc: Document = test_graph().into();
    let node = doc.graph.iter().next().unwrap().id;
    // Tiny budget so a handful of small entries overflow it.
    let mut stack = ActionStack::new(256);

    // Many distinct, non-coalescing selection edits (toggle one node
    // in/out — `from != to` each time, gesture key `None`).
    for i in 0..200 {
        let to: BTreeSet<_> = if i % 2 == 0 {
            [node].into_iter().collect()
        } else {
            BTreeSet::new()
        };
        push_edit(&mut stack, &mut doc, Intent::SetSelection { to });
        // The *live* region stays within budget (entries are far
        // smaller than 256 B, so no single-entry overflow)...
        let live = stack.actions.len() - stack.head;
        assert!(
            live <= stack.max_bytes,
            "live {live} exceeded budget {} after push {i}",
            stack.max_bytes,
        );
        // ...and the dead-prefix reclaim keeps the physical buffer
        // bounded (lazy compaction fires at head > budget).
        assert!(
            stack.actions.len() <= 2 * stack.max_bytes,
            "physical buffer {} exceeded 2× budget after push {i}",
            stack.actions.len(),
        );
    }

    // Old entries were dropped (not all 200 kept) and the newest is
    // still undoable.
    assert!(
        stack.entries.len() < 200,
        "oldest entries should have been trimmed"
    );
    assert!(
        stack.undo(&mut doc, &mut |_| {}),
        "the most recent edit stays undoable"
    );
}
