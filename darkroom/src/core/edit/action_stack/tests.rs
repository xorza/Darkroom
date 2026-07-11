use super::*;
use crate::core::document::{Document, TabRef};
use crate::core::edit::intent::{DockIntent, Intent, apply_step, build_step};
use scenarium::graph::NodeSearch;
use scenarium::graph::subgraph::SubgraphId;
use scenarium::testing::test_graph;

/// Three tabs with distinct `Local` targets in the primary group so an
/// activation/close at a given index is observable. Dock steps are
/// document-global (they only touch the layout, never resolve a graph),
/// so the fabricated subgraph ids need no backing graph.
fn doc_with_distinct_tabs() -> Document {
    let a: SubgraphId = "11111111-1111-1111-1111-111111111111".into();
    let b: SubgraphId = "22222222-2222-2222-2222-222222222222".into();
    let mut doc: Document = test_graph().into();
    let primary = doc.layout.primary().id;
    doc.layout
        .insert_tab(primary, TabRef::Graph(GraphRef::Local(a)));
    doc.layout
        .insert_tab(primary, TabRef::Graph(GraphRef::Local(b)));
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
fn dock(stack: &mut ActionStack, doc: &mut Document, op: DockIntent) -> bool {
    let step = build_step(Intent::Dock(op), doc, GraphRef::Main).unwrap();
    if step.is_noop() {
        return false;
    }
    apply_step(&step, doc, GraphRef::Main);
    stack.push_current(GraphRef::Main, &[step]);
    true
}

fn switch_to(stack: &mut ActionStack, doc: &mut Document, to: usize) {
    let group = doc.layout.primary().id;
    dock(stack, doc, DockIntent::ActivateTab { group, index: to });
}

fn close_at(stack: &mut ActionStack, doc: &mut Document, index: usize) -> bool {
    let group = doc.layout.primary().id;
    dock(stack, doc, DockIntent::CloseTab { group, index })
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
    stack.undo(&mut doc, &mut |_| {});
    assert_eq!(primary_active(&doc), 0);

    assert!(stack.redo(&mut doc, &mut |_| {}));
    assert_eq!(
        primary_active(&doc),
        2,
        "redo restores the merged switch target"
    );
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
    let mut want = std::collections::BTreeSet::new();
    want.insert(node_id);
    let sel = build_step(Intent::SetSelection { to: want }, &doc, GraphRef::Main).unwrap();
    apply_step(&sel, &mut doc, GraphRef::Main);
    stack.push_current(GraphRef::Main, &[sel]);

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
fn close_is_dropped_for_main_or_out_of_range() {
    let mut doc = doc_with_distinct_tabs();
    let mut stack = ActionStack::new(1 << 20);
    // Main (index 0) is never closable; index 3 is past the end.
    assert!(!close_at(&mut stack, &mut doc, 0), "Main must not close");
    assert!(!close_at(&mut stack, &mut doc, 3), "OOB index must drop");
    assert_eq!(primary_tabs(&doc).len(), 3, "no tab removed");
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
fn consecutive_moves_coalesce_keeping_first_from() {
    use glam::Vec2;

    let mut doc: Document = test_graph().into();
    let node = doc.graph.iter().next().unwrap().id;
    let start = doc.main_view.view_nodes.by_key(&node).unwrap().pos;
    let mut stack = ActionStack::new(1 << 20);

    let drag_to = |stack: &mut ActionStack, doc: &mut Document, to: Vec2| {
        let intent = Intent::MoveNodes {
            grabbed: node,
            to: vec![(node, to)],
        };
        let step = build_step(intent, doc, GraphRef::Main).unwrap();
        apply_step(&step, doc, GraphRef::Main);
        stack.push_current(GraphRef::Main, &[step]);
    };
    drag_to(&mut stack, &mut doc, Vec2::new(10.0, 10.0));
    drag_to(&mut stack, &mut doc, Vec2::new(20.0, 20.0));

    // Both moves of the same node collapsed into one entry: a single
    // undo restores the *original* position (the first `from`)...
    assert!(stack.undo(&mut doc, &mut |_| {}));
    assert_eq!(
        doc.main_view.view_nodes.by_key(&node).unwrap().pos,
        start,
        "one undo reverts the whole drag"
    );
    assert!(
        !stack.undo(&mut doc, &mut |_| {}),
        "the drag collapsed to exactly one entry"
    );
    // ...and redo replays to the last `to`.
    assert!(stack.redo(&mut doc, &mut |_| {}));
    assert_eq!(
        doc.main_view.view_nodes.by_key(&node).unwrap().pos,
        Vec2::new(20.0, 20.0),
    );
}

#[test]
fn moves_of_different_nodes_do_not_coalesce() {
    use glam::Vec2;

    let mut doc: Document = test_graph().into();
    let a = doc.graph.iter().next().unwrap().id;
    let b = doc.graph.iter().nth(1).unwrap().id;
    let mut stack = ActionStack::new(1 << 20);

    for (node, to) in [(a, Vec2::new(5.0, 5.0)), (b, Vec2::new(6.0, 6.0))] {
        let intent = Intent::MoveNodes {
            grabbed: node,
            to: vec![(node, to)],
        };
        let step = build_step(intent, &doc, GraphRef::Main).unwrap();
        apply_step(&step, &mut doc, GraphRef::Main);
        stack.push_current(GraphRef::Main, &[step]);
    }
    // Different grabbed nodes ⇒ different `NodeDrag` keys ⇒ two entries.
    assert!(stack.undo(&mut doc, &mut |_| {}));
    assert!(
        stack.undo(&mut doc, &mut |_| {}),
        "moves of distinct nodes stay separate undo entries"
    );
}

#[test]
fn deleting_selection_restores_nodes_and_edge_in_one_undo() {
    use scenarium::graph::{Binding, InputPort};

    let mut doc: Document = test_graph().into();
    let a = doc.graph.iter().next().unwrap().id;
    let b = doc.graph.iter().nth(1).unwrap().id;
    // Edge a -> b, then select both for deletion.
    doc.graph
        .set_input_binding(InputPort::new(b, 0), (a, 0).into());
    doc.main_view.selected_nodes = [a, b].into_iter().collect();

    // Mirror `drain_intents`: build each `RemoveNode` against the live
    // doc, apply immediately, collect into one batch entry. The a->b
    // edge is captured by a's step (before a is removed), so a single
    // undo can restore it once both nodes are back.
    let mut stack = ActionStack::new(1 << 20);
    let mut batch = Vec::new();
    for node_id in [a, b] {
        let step = build_step(Intent::RemoveNode { node_id }, &doc, GraphRef::Main).unwrap();
        apply_step(&step, &mut doc, GraphRef::Main);
        batch.push(step);
    }
    stack.push_current(GraphRef::Main, &batch);

    assert!(doc.graph.find_node(&a, NodeSearch::TopLevel).is_none());
    assert!(doc.graph.find_node(&b, NodeSearch::TopLevel).is_none());

    assert!(stack.undo(&mut doc, &mut |_| {}));
    assert!(doc.graph.find_node(&a, NodeSearch::TopLevel).is_some());
    assert!(doc.graph.find_node(&b, NodeSearch::TopLevel).is_some());
    match doc.graph.input_binding(InputPort::new(b, 0)) {
        Binding::Bind(src) => assert_eq!((src.node_id, src.port_idx), (a, 0)),
        other => panic!("expected restored a->b edge, got {other:?}"),
    }
    assert!(
        !stack.undo(&mut doc, &mut |_| {}),
        "the whole delete collapsed to one undo entry"
    );
}

#[test]
fn group_drag_moves_all_and_undoes_as_one() {
    use glam::Vec2;

    let mut doc: Document = test_graph().into();
    let a = doc.graph.iter().next().unwrap().id;
    let b = doc.graph.iter().nth(1).unwrap().id;
    let a0 = doc.main_view.view_nodes.by_key(&a).unwrap().pos;
    let b0 = doc.main_view.view_nodes.by_key(&b).unwrap().pos;
    let mut stack = ActionStack::new(1 << 20);

    // Two frames of a group drag (grabbed = a), each frame moving both
    // a and b by the running offset. Same grabbed ⇒ one coalesced entry.
    let drag = |stack: &mut ActionStack, doc: &mut Document, off: Vec2| {
        let intent = Intent::MoveNodes {
            grabbed: a,
            to: vec![(a, a0 + off), (b, b0 + off)],
        };
        let step = build_step(intent, doc, GraphRef::Main).unwrap();
        apply_step(&step, doc, GraphRef::Main);
        stack.push_current(GraphRef::Main, &[step]);
    };
    drag(&mut stack, &mut doc, Vec2::new(10.0, 0.0));
    drag(&mut stack, &mut doc, Vec2::new(25.0, 5.0));

    // Both ended at origin + last offset.
    assert_eq!(
        doc.main_view.view_nodes.by_key(&a).unwrap().pos,
        a0 + Vec2::new(25.0, 5.0)
    );
    assert_eq!(
        doc.main_view.view_nodes.by_key(&b).unwrap().pos,
        b0 + Vec2::new(25.0, 5.0)
    );

    // One undo restores BOTH to their pre-drag positions (first `from`).
    assert!(stack.undo(&mut doc, &mut |_| {}));
    assert_eq!(doc.main_view.view_nodes.by_key(&a).unwrap().pos, a0);
    assert_eq!(doc.main_view.view_nodes.by_key(&b).unwrap().pos, b0);
    assert!(
        !stack.undo(&mut doc, &mut |_| {}),
        "the group drag collapsed to exactly one entry"
    );
}

#[test]
fn new_edit_discards_the_redo_tail() {
    use std::collections::BTreeSet;

    let mut doc: Document = test_graph().into();
    let node = doc.graph.iter().next().unwrap().id;
    let mut stack = ActionStack::new(1 << 20);

    let select = |stack: &mut ActionStack, doc: &mut Document, set: BTreeSet<_>| {
        let step = build_step(Intent::SetSelection { to: set }, doc, GraphRef::Main).unwrap();
        apply_step(&step, doc, GraphRef::Main);
        stack.push_current(GraphRef::Main, &[step]);
    };
    let one: BTreeSet<_> = [node].into_iter().collect();
    select(&mut stack, &mut doc, one.clone()); // A: {} -> {node}
    select(&mut stack, &mut doc, BTreeSet::new()); // B: {node} -> {}

    // Undo B → selection back to {node}, B now redoable.
    assert!(stack.undo(&mut doc, &mut |_| {}));
    // A fresh edit while a redo is pending must discard it.
    select(&mut stack, &mut doc, BTreeSet::new()); // C: {node} -> {}
    assert!(
        !stack.redo(&mut doc, &mut |_| {}),
        "a new edit invalidates the redoable tail"
    );
}

#[test]
fn consecutive_closes_do_not_coalesce() {
    // Each close is its own undo entry — two closes need two undos.
    let mut doc = doc_with_distinct_tabs();
    let mut stack = ActionStack::new(1 << 20);

    close_at(&mut stack, &mut doc, 2);
    close_at(&mut stack, &mut doc, 1);
    assert_eq!(primary_tabs(&doc).len(), 1, "both subgraph tabs closed");

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
fn history_bounded_by_byte_budget() {
    use std::collections::BTreeSet;

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
        let step = build_step(Intent::SetSelection { to }, &doc, GraphRef::Main).unwrap();
        apply_step(&step, &mut doc, GraphRef::Main);
        stack.push_current(GraphRef::Main, &[step]);
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

/// A document carrying a subgraph def "S" with interface inputs
/// `[A]` and outputs `[R]`, plus that `Local` target.
fn doc_with_def() -> (Document, GraphRef) {
    use scenarium::data::DataType;
    use scenarium::graph::subgraph::SubgraphDef;
    use scenarium::node::function::{FuncInput, FuncOutput};

    let mut doc: Document = test_graph().into();
    let def = SubgraphDef::new("00000000-0000-0000-0000-0000000000bb", "S")
        .category("Subgraph")
        .input(FuncInput::optional("A", DataType::Int))
        .output(FuncOutput::new("R", DataType::Int));
    let id = def.id;
    doc.graph.subgraphs.add(def);
    (doc, GraphRef::Local(id))
}

#[test]
fn rename_boundary_port_applies_and_reverts() {
    use crate::core::document::BoundarySide;
    use crate::core::edit::intent::revert_step;

    let (mut doc, target) = doc_with_def();
    let GraphRef::Local(def_id) = target else {
        unreachable!()
    };
    let step = build_step(
        Intent::RenameBoundaryPort {
            side: BoundarySide::Input,
            idx: 0,
            to: "alpha".into(),
        },
        &doc,
        target,
    )
    .expect("rename builds against a Local target");

    apply_step(&step, &mut doc, target);
    assert_eq!(
        doc.graph.subgraphs.by_key(&def_id).unwrap().inputs[0].name,
        "alpha"
    );

    revert_step(&step, &mut doc, target);
    assert_eq!(
        doc.graph.subgraphs.by_key(&def_id).unwrap().inputs[0].name,
        "A",
        "revert restores the captured `from` name"
    );
}

#[test]
fn rename_boundary_port_renames_outputs_side() {
    use crate::core::document::BoundarySide;

    let (mut doc, target) = doc_with_def();
    let GraphRef::Local(def_id) = target else {
        unreachable!()
    };
    let step = build_step(
        Intent::RenameBoundaryPort {
            side: BoundarySide::Output,
            idx: 0,
            to: "result".into(),
        },
        &doc,
        target,
    )
    .unwrap();
    apply_step(&step, &mut doc, target);
    assert_eq!(
        doc.graph.subgraphs.by_key(&def_id).unwrap().outputs[0].name,
        "result"
    );
}

#[test]
fn rename_boundary_port_dropped_off_local_target_or_oob() {
    use crate::core::document::BoundarySide;

    let (doc, target) = doc_with_def();
    // Main target has no def interface to rename.
    assert!(
        build_step(
            Intent::RenameBoundaryPort {
                side: BoundarySide::Input,
                idx: 0,
                to: "x".into(),
            },
            &doc,
            GraphRef::Main,
        )
        .is_none()
    );
    // Out-of-range index on the right target also drops.
    assert!(
        build_step(
            Intent::RenameBoundaryPort {
                side: BoundarySide::Input,
                idx: 9,
                to: "x".into(),
            },
            &doc,
            target,
        )
        .is_none()
    );
}

#[test]
fn rename_undo_survives_interface_compaction() {
    use crate::core::document::BoundarySide;
    use crate::core::edit::intent::revert_step;
    use scenarium::data::DataType;
    use scenarium::graph::subgraph::SubgraphDef;
    use scenarium::node::function::FuncInput;

    let finput = |n: &str| FuncInput::optional(n, DataType::Int);
    let mut doc: Document = test_graph().into();
    let def = SubgraphDef::new("00000000-0000-0000-0000-0000000000cc", "S")
        .category("Subgraph")
        .inputs([finput("A"), finput("B")]);
    let def_id = def.id;
    doc.graph.subgraphs.add(def);
    let target = GraphRef::Local(def_id);

    // Rename inputs[1] "B" -> "beta".
    let step = build_step(
        Intent::RenameBoundaryPort {
            side: BoundarySide::Input,
            idx: 1,
            to: "beta".into(),
        },
        &doc,
        target,
    )
    .unwrap();
    apply_step(&step, &mut doc, target);
    assert_eq!(
        doc.graph.subgraphs.by_key(&def_id).unwrap().inputs[1].name,
        "beta"
    );

    // Simulate `reconcile_boundaries` compacting after input 0 ("A")
    // was disconnected: the survivor "beta" shifts from index 1 to 0.
    doc.graph
        .subgraphs
        .by_key_mut(&def_id)
        .unwrap()
        .inputs
        .remove(0);
    assert_eq!(
        doc.graph.subgraphs.by_key(&def_id).unwrap().inputs[0].name,
        "beta"
    );

    // Undo: the step's `idx` (1) is now stale, but it resolves "beta"
    // by name at its new index 0 and restores "B" — not a no-op, not
    // the wrong slot.
    revert_step(&step, &mut doc, target);
    assert_eq!(
        doc.graph.subgraphs.by_key(&def_id).unwrap().inputs[0].name,
        "B"
    );
}
