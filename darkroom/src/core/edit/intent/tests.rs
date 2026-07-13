use std::collections::BTreeSet;

use glam::Vec2;
use scenarium::data::StaticValue;
use scenarium::graph::{
    Binding, CacheMode, InputPort, Node, NodeId, NodeKind, NodeSearch, OutputPort,
};
use scenarium::node::function::FuncId;

use crate::core::document::dock::DockOp;
use crate::core::document::view_node::ViewNode;
use crate::core::document::{Document, GraphRef, SelectionKey, Viewport};
use crate::core::edit::intent::apply::{apply_step, commit_intent, revert_step};
use crate::core::edit::intent::build::build_step;
use crate::core::edit::intent::duplicate::{
    DUPLICATE_OFFSET, build_duplicate_intent, build_duplicate_intent_for, remove_selection_intents,
    selected_node_ids,
};
use crate::core::edit::intent::types::{
    DocStep, GestureKey, GraphStep, Intent, NodeProperty, UndoStep,
};

/// Add a bare `Func`-kind node to `doc`'s root graph + main view at
/// `pos`, returning its id.
fn add_node_at(doc: &mut Document, pos: Vec2) -> NodeId {
    let node = Node::new(NodeKind::Func(FuncId::unique()));
    let id = node.id;
    doc.graph.add(node);
    doc.main_view.view_nodes.add(ViewNode { id, pos });
    id
}

#[test]
fn dirties_document_splits_edits_from_navigation() {
    use crate::core::document::TabRef;
    use crate::core::document::dock::{DockDrop, SplitSide};
    use scenarium::graph::subgraph::SubgraphId;

    // A doc with a movable Preferences tab, for the dock steps below
    // (both built through the real `build_step` pipeline so the
    // `structural` derivation is what's under test).
    let mut dock_doc = Document::default();
    let primary = dock_doc.layout.primary().id;
    dock_doc.layout.find_or_insert(TabRef::Preferences, primary);
    let dock_step = |op: DockOp| build_step(Intent::Dock(op), &dock_doc, GraphRef::Main);

    // Navigation-only steps: camera, selection, tab focus — the user
    // doesn't "save" these, so they must not flip the unsaved flag.
    let navigation = [
        UndoStep::Graph(GraphStep::SetSelection {
            from: BTreeSet::new(),
            to: BTreeSet::from([SelectionKey::Node(NodeId::unique())]),
        }),
        UndoStep::Graph(GraphStep::SetViewport {
            from: Viewport {
                pan: Vec2::ZERO,
                zoom: 1.0,
            },
            to: Viewport {
                pan: Vec2::new(10.0, 20.0),
                zoom: 2.0,
            },
        }),
        // Activating a tab is focus, not arrangement work.
        dock_step(DockOp::ActivateTab {
            group: primary,
            index: 1,
        })
        .unwrap(),
    ];
    for step in &navigation {
        assert!(
            !step.dirties_document(),
            "navigation step must not dirty: {step:?}",
        );
    }

    // Content steps: graph data + node layout — real, savable work.
    let content = [
        UndoStep::Graph(GraphStep::RenameNode {
            node_id: NodeId::unique(),
            from: "a".into(),
            to: "b".into(),
        }),
        // Splitting a tab into its own pane is invested arrangement
        // work — the exit prompt should protect it.
        dock_step(DockOp::MoveTab {
            tab: TabRef::Preferences,
            to: DockDrop::Split {
                group: primary,
                side: SplitSide::Right,
            },
        })
        .unwrap(),
        UndoStep::Graph(GraphStep::MoveSelection {
            grabbed: SelectionKey::Node(NodeId::unique()),
            node_moves: vec![(NodeId::unique(), Vec2::ZERO, Vec2::new(5.0, 5.0))],
            pin_moves: vec![],
        }),
        UndoStep::Doc(DocStep::RenameSubgraph {
            id: SubgraphId::unique(),
            from: "s".into(),
            to: "t".into(),
        }),
    ];
    for step in &content {
        assert!(step.dirties_document(), "content step must dirty: {step:?}",);
    }
}

#[test]
fn subscribe_unsubscribe_commit_and_undo() {
    let mut doc = Document::default();
    let emitter = add_node_at(&mut doc, Vec2::ZERO);
    let subscriber = add_node_at(&mut doc, Vec2::new(100.0, 0.0));
    let set_sub = |e, i, s, subscribe| Intent::SetSubscription {
        emitter: e,
        event_idx: i,
        subscriber: s,
        subscribe,
    };

    // Subscribe commits and writes the edge.
    let step = commit_intent(
        set_sub(emitter, 0, subscriber, true),
        &mut doc,
        GraphRef::Main,
    )
    .expect("subscribe commits");
    assert!(doc.graph.is_subscribed(emitter, 0, subscriber));

    // A second identical subscribe is a no-op (from == to == true).
    assert!(
        commit_intent(
            set_sub(emitter, 0, subscriber, true),
            &mut doc,
            GraphRef::Main
        )
        .is_none(),
        "re-subscribing the same edge is a no-op"
    );

    // Undo removes it; redo restores it.
    revert_step(&step, &mut doc, GraphRef::Main);
    assert!(!doc.graph.is_subscribed(emitter, 0, subscriber));
    apply_step(&step, &mut doc, GraphRef::Main);
    assert!(doc.graph.is_subscribed(emitter, 0, subscriber));

    // Unsubscribe commits, removes the edge, and undo brings it back.
    let step = commit_intent(
        set_sub(emitter, 0, subscriber, false),
        &mut doc,
        GraphRef::Main,
    )
    .expect("unsubscribe commits");
    assert!(!doc.graph.is_subscribed(emitter, 0, subscriber));
    revert_step(&step, &mut doc, GraphRef::Main);
    assert!(doc.graph.is_subscribed(emitter, 0, subscriber));

    // Redo the unsubscribe (apply writes the `to = unsubscribed` half),
    // then unsubscribing the now-absent edge is a no-op.
    apply_step(&step, &mut doc, GraphRef::Main);
    assert!(!doc.graph.is_subscribed(emitter, 0, subscriber));
    assert!(
        commit_intent(
            set_sub(emitter, 0, subscriber, false),
            &mut doc,
            GraphRef::Main
        )
        .is_none(),
        "unsubscribing a missing edge is a no-op"
    );
}

#[test]
fn subscribe_to_missing_node_is_dropped() {
    let mut doc = Document::default();
    let emitter = add_node_at(&mut doc, Vec2::ZERO);
    let ghost = NodeId::unique();
    assert!(
        commit_intent(
            Intent::SetSubscription {
                emitter,
                event_idx: 0,
                subscriber: ghost,
                subscribe: true,
            },
            &mut doc,
            GraphRef::Main,
        )
        .is_none(),
        "a subscription to a node that doesn't exist is dropped, not recorded"
    );
}

#[test]
fn duplicate_intent_drops_or_keeps_external_by_flag() {
    // a -> b (internal edge, both selected); c -> b (external, c not
    // selected). b also has a Const on input 1. Selecting {a, b} must
    // duplicate a' and b', keep a'->b' and the Const, drop c->b.
    let mut doc = Document::default();
    let a = add_node_at(&mut doc, Vec2::new(0.0, 0.0));
    let b = add_node_at(&mut doc, Vec2::new(100.0, 0.0));
    let c = add_node_at(&mut doc, Vec2::new(0.0, 100.0));
    doc.graph
        .set_input_binding(InputPort::new(b, 0), Binding::bind(a, 0));
    doc.graph.set_input_binding(
        InputPort::new(b, 1),
        Binding::Const(StaticValue::from(7i64)),
    );
    doc.graph
        .set_input_binding(InputPort::new(b, 2), Binding::bind(c, 0));
    let node_ids: BTreeSet<NodeId> = [a, b].into_iter().collect();
    doc.main_view.selected = node_ids.iter().copied().map(SelectionKey::Node).collect();

    let Some(Intent::DuplicateNodes {
        nodes,
        bindings,
        subscriptions,
    }) = build_duplicate_intent(&doc, GraphRef::Main)
    else {
        panic!("expected a DuplicateNodes intent");
    };

    assert_eq!(nodes.len(), 2, "both selected nodes cloned");
    assert!(subscriptions.is_empty());
    // Fresh ids, offset positions.
    let new_ids: BTreeSet<SelectionKey> = nodes
        .iter()
        .map(|(_, n)| SelectionKey::Node(n.id))
        .collect();
    assert!(
        new_ids.is_disjoint(&doc.main_view.selected),
        "clones get fresh ids"
    );
    let a_clone = nodes
        .iter()
        .find(|(vn, _)| vn.pos == Vec2::new(0.0, 0.0) + DUPLICATE_OFFSET)
        .map(|(_, n)| n.id)
        .expect("a's clone offset from its origin");

    // Exactly two bindings survive: the internal a'->b' edge and the
    // Const; the external c->b edge (input 2) is gone.
    assert_eq!(bindings.len(), 2);
    let b_clone = nodes
        .iter()
        .find(|(vn, _)| vn.pos == Vec2::new(100.0, 0.0) + DUPLICATE_OFFSET)
        .map(|(_, n)| n.id)
        .unwrap();
    let internal = bindings
        .iter()
        .find(|(port, _)| port.port_idx == 0)
        .expect("a'->b' edge present");
    assert_eq!(internal.0.node_id, b_clone, "edge sinks into b's clone");
    match &internal.1 {
        Binding::Bind(src) => {
            assert_eq!(src.node_id, a_clone, "remapped to a's clone");
            assert_eq!(src.port_idx, 0);
        }
        other => panic!("expected Bind, got {other:?}"),
    }
    assert!(
        bindings
            .iter()
            .any(|(port, bind)| port.port_idx == 1 && matches!(bind, Binding::Const(_))),
        "const binding copied"
    );
    assert!(
        !bindings.iter().any(|(port, _)| port.port_idx == 2),
        "external edge dropped"
    );

    // With `include_incoming`, the same selection keeps the external
    // c -> b edge, the clone's input still pointing at the original c.
    // (Fresh build → fresh clone ids, so re-find b's clone by position.)
    let Some(Intent::DuplicateNodes {
        nodes: incoming_nodes,
        bindings: incoming,
        ..
    }) = build_duplicate_intent_for(&doc, GraphRef::Main, &node_ids, true)
    else {
        panic!("expected a DuplicateNodes intent");
    };
    assert_eq!(incoming.len(), 3, "internal + const + kept external");
    let b_clone2 = incoming_nodes
        .iter()
        .find(|(vn, _)| vn.pos == Vec2::new(100.0, 0.0) + DUPLICATE_OFFSET)
        .map(|(_, n)| n.id)
        .unwrap();
    let external = incoming
        .iter()
        .find(|(port, _)| port.port_idx == 2)
        .expect("external edge kept");
    assert_eq!(external.0.node_id, b_clone2, "edge sinks into b's clone");
    match &external.1 {
        Binding::Bind(src) => {
            assert_eq!(src.node_id, c, "external source stays the original c");
            assert_eq!(src.port_idx, 0);
        }
        other => panic!("expected Bind, got {other:?}"),
    }
}

#[test]
fn duplicate_intent_none_without_selection() {
    let mut doc = Document::default();
    add_node_at(&mut doc, Vec2::ZERO);
    assert!(build_duplicate_intent(&doc, GraphRef::Main).is_none());

    // A selection of only pin previews has no node identity to clone —
    // same as an empty selection.
    let id = add_node_at(&mut doc, Vec2::new(50.0, 0.0));
    doc.main_view.selected = [SelectionKey::Pin(OutputPort::new(id, 0))]
        .into_iter()
        .collect();
    assert!(
        build_duplicate_intent(&doc, GraphRef::Main).is_none(),
        "pin-only selection has no node to duplicate"
    );
}

#[test]
fn selected_node_ids_drops_pin_keys() {
    let mut doc = Document::default();
    let a = add_node_at(&mut doc, Vec2::ZERO);
    let b = add_node_at(&mut doc, Vec2::new(50.0, 0.0));
    doc.main_view.selected = [
        SelectionKey::Node(a),
        SelectionKey::Pin(OutputPort::new(b, 0)),
    ]
    .into_iter()
    .collect();

    let view = doc.scope(GraphRef::Main).unwrap().view;
    assert_eq!(
        selected_node_ids(view),
        BTreeSet::from([a]),
        "only the node key survives; the pin key carries no node identity"
    );
}

#[test]
fn remove_selection_intents_splits_nodes_from_pins() {
    let node_id = NodeId::unique();
    let port = OutputPort::new(NodeId::unique(), 2);
    let selected: BTreeSet<SelectionKey> = [SelectionKey::Node(node_id), SelectionKey::Pin(port)]
        .into_iter()
        .collect();

    let mut intents = remove_selection_intents(&selected);
    assert_eq!(intents.len(), 2);
    intents.sort_by_key(|i| matches!(i, Intent::SetOutputPinned { .. }));

    assert!(matches!(
        intents[0],
        Intent::RemoveNode { node_id: id } if id == node_id
    ));
    assert!(matches!(
        intents[1],
        Intent::SetOutputPinned { output, pinned: false }
            if output == port
    ));
}

#[test]
fn set_node_property_commits_and_reverts() {
    let mut doc = Document::default();
    let id = add_node_at(&mut doc, Vec2::ZERO);
    // Fresh nodes default to no caching (None) and enabled.
    assert_eq!(
        doc.graph
            .find_node(&id, NodeSearch::TopLevel)
            .unwrap()
            .cache,
        CacheMode::None
    );
    assert!(
        !doc.graph
            .find_node(&id, NodeSearch::TopLevel)
            .unwrap()
            .disabled
    );

    // Both properties ride the one `SetNodeProperty` path. A representative flip
    // each (the cache header chips: None→Both/Ram/Disk; the disable chip: →on),
    // committing then reverting — each iteration returns the node to its defaults,
    // so the step's captured `from` is always None / enabled.
    let cases = [
        NodeProperty::RuntimeCache(CacheMode::Both),
        NodeProperty::RuntimeCache(CacheMode::Ram),
        NodeProperty::RuntimeCache(CacheMode::Disk),
        NodeProperty::Disabled(true),
    ];
    for to in cases {
        let step = commit_intent(
            Intent::SetNodeProperty { node_id: id, to },
            &mut doc,
            GraphRef::Main,
        )
        .unwrap_or_else(|| panic!("{to:?} is a real change, not a no-op"));
        let node = doc.graph.find_node(&id, NodeSearch::TopLevel).unwrap();
        match to {
            NodeProperty::RuntimeCache(m) => assert_eq!(node.cache, m),
            NodeProperty::Disabled(d) => assert_eq!(node.disabled, d),
        }
        assert!(
            !step.requires_relayout() && !step.requires_reconcile(),
            "a node-property toggle neither remeasures nor reshapes the interface"
        );
        assert!(
            step.gesture_key().is_none(),
            "each toggle is its own undo entry"
        );
        revert_step(&step, &mut doc, GraphRef::Main);
        let node = doc.graph.find_node(&id, NodeSearch::TopLevel).unwrap();
        assert_eq!(node.cache, CacheMode::None, "revert restores the cache");
        assert!(!node.disabled, "revert restores the disable flag");
    }

    // Setting a property to the value it already holds is a no-op (no undo entry).
    for to in [
        NodeProperty::RuntimeCache(CacheMode::None),
        NodeProperty::Disabled(false),
    ] {
        assert!(
            commit_intent(
                Intent::SetNodeProperty { node_id: id, to },
                &mut doc,
                GraphRef::Main,
            )
            .is_none(),
            "{to:?} equals the current value → writes nothing"
        );
    }
}

#[test]
fn set_output_pinned_commits_reverts_and_no_ops() {
    let mut doc = Document::default();
    let id = add_node_at(&mut doc, Vec2::ZERO);
    let port = OutputPort::new(id, 0);
    assert!(!doc.graph.is_output_pinned(port));

    let step = commit_intent(
        Intent::SetOutputPinned {
            output: port,
            pinned: true,
        },
        &mut doc,
        GraphRef::Main,
    )
    .expect("marking an unbound port is a real change");
    assert!(doc.graph.is_output_pinned(port));
    assert_eq!(
        doc.main_view.pin_positions.get(&port),
        Some(&Vec2::ZERO),
        "pinning seeds an explicit position — no unset/sparse state"
    );
    assert!(
        !step.requires_relayout() && !step.requires_reconcile(),
        "a pin toggle neither remeasures nor reshapes the interface"
    );
    assert!(step.dirties_document(), "a real graph edit worth saving");
    assert!(
        step.gesture_key().is_none(),
        "each toggle is its own undo entry"
    );

    revert_step(&step, &mut doc, GraphRef::Main);
    assert!(!doc.graph.is_output_pinned(port), "revert clears it");
    apply_step(&step, &mut doc, GraphRef::Main);
    assert!(doc.graph.is_output_pinned(port), "redo re-marks it");

    // Selecting the pin, then unpinning it, drops the selection — its
    // preview widget is gone; reverting the unpin restores it (mirrors
    // `RemoveNode`'s `was_selected`).
    doc.main_view.selected.insert(SelectionKey::Pin(port));
    let unpin = commit_intent(
        Intent::SetOutputPinned {
            output: port,
            pinned: false,
        },
        &mut doc,
        GraphRef::Main,
    )
    .expect("unpinning a pinned port is a real change");
    assert!(!doc.graph.is_output_pinned(port));
    assert!(
        !doc.main_view.selected.contains(&SelectionKey::Pin(port)),
        "unpinning drops the now-gone widget's selection"
    );
    revert_step(&unpin, &mut doc, GraphRef::Main);
    assert!(doc.graph.is_output_pinned(port), "revert re-pins it");
    assert!(
        doc.main_view.selected.contains(&SelectionKey::Pin(port)),
        "revert restores the selection the pin had before it was unpinned"
    );
    assert_eq!(
        doc.main_view.pin_positions.get(&port),
        Some(&Vec2::ZERO),
        "unpinning leaves the position entry in place rather than pruning it"
    );

    // Setting to the value it already holds is a no-op (no undo entry).
    assert!(
        commit_intent(
            Intent::SetOutputPinned {
                output: port,
                pinned: true,
            },
            &mut doc,
            GraphRef::Main,
        )
        .is_none(),
        "already bound → writes nothing"
    );
}

#[test]
fn set_output_pinned_on_missing_node_is_dropped() {
    // The GUI only ever targets a port rendered in the current frame's
    // Scene, but a script's generic `apply()` reaches this variant
    // unchecked too — a bogus or stale `node_id` must drop quietly
    // rather than crash the whole process.
    let mut doc = Document::default();
    let step = commit_intent(
        Intent::SetOutputPinned {
            output: OutputPort::new(NodeId::unique(), 0),
            pinned: true,
        },
        &mut doc,
        GraphRef::Main,
    );
    assert!(step.is_none());
}

/// A lone-pin `MoveSelection` intent: `grabbed`/`pins` target `port`,
/// no nodes in the group.
fn move_pin(port: OutputPort, to: Vec2) -> Intent {
    Intent::MoveSelection {
        grabbed: SelectionKey::Pin(port),
        nodes: vec![],
        pins: vec![(port, to)],
    }
}

#[test]
fn move_selection_repositions_a_pin_commits_reverts_and_coalesces() {
    let mut doc = Document::default();
    let id = add_node_at(&mut doc, Vec2::ZERO);
    let port = OutputPort::new(id, 0);

    // Pinning seeds a zero-default position — every pinned port has an
    // explicit entry from the moment it's pinned, no unset/sparse state.
    commit_intent(
        Intent::SetOutputPinned {
            output: port,
            pinned: true,
        },
        &mut doc,
        GraphRef::Main,
    )
    .expect("pinning is a real change");
    assert_eq!(doc.main_view.pin_positions.get(&port), Some(&Vec2::ZERO));

    let step = commit_intent(
        move_pin(port, Vec2::new(30.0, -12.0)),
        &mut doc,
        GraphRef::Main,
    )
    .expect("first drag off the seeded default is a real change");
    assert_eq!(
        doc.main_view.pin_positions.get(&port),
        Some(&Vec2::new(30.0, -12.0))
    );
    assert!(
        !step.requires_relayout() && !step.requires_reconcile(),
        "repositioning a decoration (no nodes in the group) neither remeasures nor reshapes the interface"
    );
    assert!(step.dirties_document(), "a real, persisted edit");
    assert_eq!(
        step.gesture_key(),
        Some(GestureKey::SelectionDrag(SelectionKey::Pin(port))),
        "consecutive frames of the same pin's drag must coalesce"
    );

    // A later frame of the same drag: coalesce keeps the original
    // `from` (the seeded zero default) and adopts the new `to`.
    let step2 = build_step(move_pin(port, Vec2::new(50.0, -20.0)), &doc, GraphRef::Main).unwrap();
    apply_step(&step2, &mut doc, GraphRef::Main);
    let merged = step.coalesce(&step2).expect("same pin ⇒ coalesces");
    assert_eq!(
        merged.gesture_key(),
        Some(GestureKey::SelectionDrag(SelectionKey::Pin(port))),
        "merged step keeps the same key"
    );

    // Reverting the *merged* step restores the original seeded (zero)
    // position rather than the drag's intermediate or final position.
    revert_step(&merged, &mut doc, GraphRef::Main);
    assert_eq!(
        doc.main_view.pin_positions.get(&port),
        Some(&Vec2::ZERO),
        "revert restores the pre-drag default, not a leftover offset"
    );

    // Dragging to the exact position it already holds is a no-op.
    doc.main_view
        .pin_positions
        .insert(port, Vec2::new(1.0, 2.0));
    assert!(
        commit_intent(
            move_pin(port, Vec2::new(1.0, 2.0)),
            &mut doc,
            GraphRef::Main
        )
        .is_none(),
        "same position → writes nothing"
    );
}

#[test]
fn move_selection_pin_on_missing_node_is_dropped() {
    // A satellite drag spans frames and can outlive its anchor (the
    // node was deleted mid-drag) — a stale target's pin move drops
    // quietly rather than asserting; an empty resulting batch is a
    // no-op, not an error.
    let mut doc = Document::default();
    let step = commit_intent(
        move_pin(OutputPort::new(NodeId::unique(), 0), Vec2::ZERO),
        &mut doc,
        GraphRef::Main,
    );
    assert!(step.is_none());
}

#[test]
fn removing_a_node_captures_and_restores_its_pin_positions() {
    let mut doc = Document::default();
    let id = add_node_at(&mut doc, Vec2::ZERO);
    let port = OutputPort::new(id, 0);
    doc.main_view
        .pin_positions
        .insert(port, Vec2::new(7.0, 8.0));

    let step = commit_intent(Intent::RemoveNode { node_id: id }, &mut doc, GraphRef::Main)
        .expect("removing an existing node is a real change");
    assert!(
        !doc.main_view.pin_positions.contains_key(&port),
        "the node's pin offset is pruned along with everything else"
    );

    revert_step(&step, &mut doc, GraphRef::Main);
    assert_eq!(
        doc.main_view.pin_positions.get(&port),
        Some(&Vec2::new(7.0, 8.0)),
        "undo restores the node's custom pin offset"
    );
}

#[test]
fn raise_node_reorders_persists_and_undoes() {
    use common::SerdeFormat;

    let mut doc = Document::default();
    let a = add_node_at(&mut doc, Vec2::ZERO);
    let b = add_node_at(&mut doc, Vec2::new(100.0, 0.0));
    let c = add_node_at(&mut doc, Vec2::new(0.0, 100.0));

    let order = |doc: &Document| -> Vec<NodeId> {
        doc.main_view.view_nodes.iter().map(|vn| vn.id).collect()
    };
    assert_eq!(order(&doc), vec![a, b, c], "seed order is insertion order");

    // Raise `a` (the back node) to the top — the end of `view_nodes`,
    // painted last and so drawn in front.
    let step = commit_intent(Intent::RaiseNode { node_id: a }, &mut doc, GraphRef::Main)
        .expect("raising a back node is a real reorder");
    assert_eq!(
        order(&doc),
        vec![b, c, a],
        "a moved to the top of the stack"
    );

    // Stacking is view-state: undoable + persisted, but not dirty-worthy,
    // and it neither remeasures nor reshapes a subgraph interface.
    assert!(
        !step.dirties_document(),
        "a bare restack shouldn't nag on save"
    );
    assert!(!step.requires_relayout());
    assert!(!step.requires_reconcile());
    assert!(
        step.gesture_key().is_none(),
        "each raise is its own undo entry"
    );

    // Undo restores the prior order; redo re-raises.
    revert_step(&step, &mut doc, GraphRef::Main);
    assert_eq!(order(&doc), vec![a, b, c], "undo restores the prior order");
    apply_step(&step, &mut doc, GraphRef::Main);
    assert_eq!(order(&doc), vec![b, c, a], "redo re-raises a");

    // Raising the node already on top writes nothing.
    assert!(
        commit_intent(Intent::RaiseNode { node_id: a }, &mut doc, GraphRef::Main).is_none(),
        "raising the frontmost node is a no-op"
    );

    // The whole point: render order round-trips through save/load.
    let bytes = doc.serialize(SerdeFormat::Rhai).unwrap();
    let reloaded = Document::deserialize(SerdeFormat::Rhai, &bytes).unwrap();
    assert_eq!(
        order(&reloaded),
        vec![b, c, a],
        "render order survives save/load"
    );
}

#[test]
fn commit_intent_rejects_cycle_forming_bind() {
    // a → b (b's input 0 bound to a's output 0).
    let mut doc = Document::default();
    let a = add_node_at(&mut doc, Vec2::ZERO);
    let b = add_node_at(&mut doc, Vec2::new(100.0, 0.0));
    let c = add_node_at(&mut doc, Vec2::new(0.0, 100.0));
    doc.graph
        .set_input_binding(InputPort::new(b, 0), Binding::bind(a, 0));

    // Wiring a's input back to b's output would close the a → b loop:
    // rejected, nothing written, the existing edge untouched.
    assert!(
        commit_intent(
            Intent::SetInput {
                input: InputPort::new(a, 0),
                to: Binding::bind(b, 0),
            },
            &mut doc,
            GraphRef::Main,
        )
        .is_none(),
        "a bind that closes a cycle is rejected"
    );
    assert_eq!(
        doc.graph.input_binding(InputPort::new(a, 0)),
        Binding::None,
        "the rejected bind left a's input unbound"
    );
    assert_eq!(
        doc.graph.input_binding(InputPort::new(b, 0)),
        Binding::bind(a, 0),
        "the existing a → b edge is untouched"
    );

    // A bind that keeps the graph acyclic still commits: c's input ← b's
    // output extends the chain into a → b → c.
    assert!(
        commit_intent(
            Intent::SetInput {
                input: InputPort::new(c, 0),
                to: Binding::bind(b, 0),
            },
            &mut doc,
            GraphRef::Main,
        )
        .is_some(),
        "an acyclic bind commits"
    );
    assert_eq!(
        doc.graph.input_binding(InputPort::new(c, 0)),
        Binding::bind(b, 0),
    );
}
