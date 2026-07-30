use scenarium::NodeId;
use std::collections::BTreeSet;

use super::*;
use crate::core::document::dock::DockOp;
use crate::core::document::{Document, TabRef};
use crate::core::edit::intent::apply::{apply_step, commit_dock_op};
use crate::core::edit::intent::build::build_step;
use crate::core::edit::intent::types::Intent;
use scenarium::testing::test_graph;

/// Three distinct tabs in the primary group so an activation/close at a
/// given index is observable. The viewer tabs are keyed by node, and dock
/// steps only touch the layout, so the fabricated node ids need no backing
/// node until a test asks the document to reconcile.
fn doc_with_distinct_tabs() -> Document {
    let mut doc: Document = test_graph().into();
    let primary = doc.layout.primary().id;
    doc.layout.find_or_insert(TabRef::Preferences, primary);
    doc.layout
        .find_or_insert(TabRef::ImageViewer(NodeId::unique()), primary);
    doc
}

fn primary_tabs(doc: &Document) -> Vec<TabRef> {
    doc.layout.primary().tabs.clone()
}

fn primary_active(doc: &Document) -> usize {
    doc.layout.primary().active
}

/// Commit a dock op through the real intent path and push it. Mirrors
/// the drain's no-op filter: a refused/degenerate op builds a
/// `from == to` step, which is dropped — `false` back to the caller.
fn dock(stack: &mut ActionStack, doc: &mut Document, op: DockOp) -> bool {
    let Ok(step) = commit_dock_op(op, doc) else {
        return false;
    };
    stack.push_current(&[step]);
    true
}

/// Ops name their tab, so these resolve the primary strip's slot at call
/// time — the index is the test's way of pointing at a tab, never part of
/// the op.
fn switch_to(stack: &mut ActionStack, doc: &mut Document, to: usize) {
    let tab = primary_tabs(doc)[to];
    dock(stack, doc, DockOp::ActivateTab { tab });
}

fn close_at(stack: &mut ActionStack, doc: &mut Document, index: usize) -> bool {
    let tab = primary_tabs(doc)[index];
    dock(stack, doc, DockOp::CloseTab { tab })
}

/// Push one graph edit through the real build/apply path, as `drain_intents`
/// does — the shape every coalescing test below repeats.
fn push_edit(stack: &mut ActionStack, doc: &mut Document, intent: Intent) {
    let step = build_step(intent, doc).unwrap();
    apply_step(&step, doc);
    stack.push_current(&[step]);
}

#[test]
fn consecutive_switches_coalesce_into_one_undo() {
    let mut doc = doc_with_distinct_tabs();
    let mut stack = ActionStack::new(1 << 20);

    switch_to(&mut stack, &mut doc, 1);
    switch_to(&mut stack, &mut doc, 2);
    assert_eq!(primary_active(&doc), 2, "active follows the latest switch");

    // The two switches merged: a single undo jumps straight back to
    // the pre-burst tab (0), not to the intermediate 1.
    assert!(stack.undo(&mut doc, &mut |_| {}));
    assert_eq!(
        primary_active(&doc),
        0,
        "one undo reverts the whole switch burst"
    );

    // No second entry survived the merge.
    assert!(
        !stack.undo(&mut doc, &mut |_| {}),
        "the burst collapsed to exactly one entry"
    );
}

#[test]
fn redo_replays_the_merged_switch() {
    let mut doc = doc_with_distinct_tabs();
    let mut stack = ActionStack::new(1 << 20);

    switch_to(&mut stack, &mut doc, 1);
    switch_to(&mut stack, &mut doc, 2);
    for _ in 0..8 {
        assert!(stack.undo(&mut doc, &mut |_| {}));
        assert_eq!(primary_active(&doc), 0);

        assert!(stack.redo(&mut doc, &mut |_| {}));
        assert_eq!(
            primary_active(&doc),
            2,
            "redo restores the merged switch target"
        );
    }
}

#[test]
fn switch_does_not_merge_across_an_intervening_edit() {
    // A non-switch entry between two switches breaks the gesture, so
    // the second switch starts a fresh, separately-undoable entry.
    let mut doc = doc_with_distinct_tabs();
    let mut stack = ActionStack::new(1 << 20);

    switch_to(&mut stack, &mut doc, 1);

    // Intervening selection edit (a real change, so not a no-op).
    let node_id = doc.graph.iter().next().unwrap().id;
    let want: BTreeSet<_> = [node_id].into_iter().collect();
    push_edit(&mut stack, &mut doc, Intent::SetSelection { to: want });

    switch_to(&mut stack, &mut doc, 2);
    assert_eq!(primary_active(&doc), 2);

    // First undo reverts only the second switch (2 → 1); it didn't
    // merge into the first because the selection edit broke the run.
    stack.undo(&mut doc, &mut |_| {});
    assert_eq!(
        primary_active(&doc),
        1,
        "switch after an edit is its own entry"
    );
}

#[test]
fn close_is_dropped_for_the_graph_pane_or_a_tab_that_is_not_open() {
    let mut doc = doc_with_distinct_tabs();
    let mut stack = ActionStack::new(1 << 20);
    // The graph pane is never closable; a tab that isn't open anywhere no-ops.
    assert!(
        !close_at(&mut stack, &mut doc, 0),
        "the graph pane must not close"
    );
    assert!(
        !dock(
            &mut stack,
            &mut doc,
            DockOp::CloseTab {
                tab: TabRef::ImageViewer(NodeId::unique())
            }
        ),
        "closing a tab that isn't open must drop"
    );
    assert_eq!(primary_tabs(&doc).len(), 3, "no tab removed");
}

#[test]
fn tab_ops_follow_their_tab_across_a_layout_change() {
    // The invariant the dock's whole click path rests on. A dock op is
    // built from one frame's chip response and applied a phase later, with
    // undo able to rearrange the strip in between. Because ops name their
    // tab rather than its slot, the rearrangement can't redirect one onto
    // whatever slid into that slot.
    let mut doc = doc_with_distinct_tabs();
    let mut stack = ActionStack::new(1 << 20);
    let [graph, _a, b] = primary_tabs(&doc)[..] else {
        panic!("seeded with three tabs");
    };

    // Built while `b` sits at slot 2, applied after `a` left and `b` slid
    // down to slot 1.
    let close_b = DockOp::CloseTab { tab: b };
    assert!(close_at(&mut stack, &mut doc, 1), "close a");
    assert_eq!(primary_tabs(&doc), [graph, b], "b moved to slot 1");

    commit_dock_op(close_b, &mut doc).expect("closing an open tab applies");
    assert_eq!(
        primary_tabs(&doc),
        [graph],
        "the op closed b, not whatever now occupies slot 2"
    );
    doc.validate().unwrap();
}

#[test]
fn close_then_undo_restores_tab_and_active() {
    let mut doc = doc_with_distinct_tabs();
    let b = primary_tabs(&doc)[2];
    let mut stack = ActionStack::new(1 << 20);
    switch_to(&mut stack, &mut doc, 2); // viewing the tab we're about to close

    assert!(close_at(&mut stack, &mut doc, 2));
    // Tab gone; active clamped from 2 into the new range [0, 1].
    assert_eq!(primary_tabs(&doc).len(), 2);
    assert_eq!(
        primary_active(&doc),
        1,
        "active clamped after closing the last tab"
    );

    // Undo reinserts the closed tab at its index and restores active —
    // the step snapshots the whole layout, so exact state comes back.
    assert!(stack.undo(&mut doc, &mut |_| {}));
    assert_eq!(primary_tabs(&doc).len(), 3);
    assert_eq!(
        primary_tabs(&doc)[2],
        b,
        "closed tab restored at its original index"
    );
    assert_eq!(
        primary_active(&doc),
        2,
        "active restored to the pre-close value"
    );
}

#[test]
fn close_left_of_cursor_keeps_active_in_range() {
    let mut doc = doc_with_distinct_tabs();
    let b = primary_tabs(&doc)[2];
    let mut stack = ActionStack::new(1 << 20);
    switch_to(&mut stack, &mut doc, 2);

    assert!(close_at(&mut stack, &mut doc, 1));
    assert_eq!(primary_tabs(&doc).len(), 2);
    // Old index 2 (`b`) is now at index 1; the clamped active still
    // points at it.
    assert_eq!(primary_active(&doc), 1);
    assert_eq!(primary_tabs(&doc)[1], b);

    stack.undo(&mut doc, &mut |_| {});
    assert_eq!(
        primary_active(&doc),
        2,
        "active restored across the reinsert"
    );
    assert_eq!(primary_tabs(&doc).len(), 3);
}

#[test]
fn close_redo_replays() {
    let mut doc = doc_with_distinct_tabs();
    let mut stack = ActionStack::new(1 << 20);
    switch_to(&mut stack, &mut doc, 1);

    close_at(&mut stack, &mut doc, 1);
    assert_eq!(primary_tabs(&doc).len(), 2);
    stack.undo(&mut doc, &mut |_| {});
    assert_eq!(primary_tabs(&doc).len(), 3);

    assert!(stack.redo(&mut doc, &mut |_| {}));
    assert_eq!(primary_tabs(&doc).len(), 2, "redo re-closes the tab");
    assert_eq!(primary_active(&doc), 1);
}

#[test]
fn consecutive_closes_do_not_coalesce() {
    // Each close is its own undo entry — two closes need two undos.
    let mut doc = doc_with_distinct_tabs();
    let mut stack = ActionStack::new(1 << 20);

    close_at(&mut stack, &mut doc, 2);
    close_at(&mut stack, &mut doc, 1);
    assert_eq!(primary_tabs(&doc).len(), 1, "both closable tabs closed");

    stack.undo(&mut doc, &mut |_| {});
    assert_eq!(primary_tabs(&doc).len(), 2, "first undo restores one tab");
    stack.undo(&mut doc, &mut |_| {});
    assert_eq!(
        primary_tabs(&doc).len(),
        3,
        "second undo restores the other"
    );
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
