use std::collections::BTreeSet;

use glam::Vec2;
use scenarium::FuncId;
use scenarium::StaticValue;
use scenarium::{Binding, CacheMode, InputPort, Node, NodeId, NodeKind, NodeSearch, OutputPort};
use scenarium::{GraphDef, GraphId, GraphLink, Subscription};

use crate::core::document::dock::DockOp;
use crate::core::document::{Document, GraphRef, ItemRef, Viewport};
use crate::core::edit::intent::apply::{apply_step, commit_intent, revert_step};
use crate::core::edit::intent::build::build_step;
use crate::core::edit::intent::duplicate::internals::duplicate_offset;
use crate::core::edit::intent::duplicate::{
    build_duplicate_intent, build_duplicate_intent_for, remove_selection_intents, selected_node_ids,
};
use crate::core::edit::intent::types::{
    DocStep, GestureKey, GraphStep, Intent, NodeProperty, Refusal, UndoStep,
};

/// Add a bare `Func`-kind node to `doc`'s root graph + main view at
/// `pos`, returning its id.
fn add_node_at(doc: &mut Document, pos: Vec2) -> NodeId {
    let node = Node::new(NodeKind::Func(FuncId::unique()));
    let id = doc.graph.add(node);
    doc.main_view.item_placements.insert(ItemRef::Node(id), pos);
    id
}

/// `port`'s preview position in the main view, `None` when it has no item
/// (i.e. isn't pinned).
fn pin_pos(doc: &Document, port: OutputPort) -> Option<Vec2> {
    doc.main_view
        .item_placements
        .get(&ItemRef::Pin(port))
        .copied()
}

/// The main view's paint-stack order, back to front.
fn stack_order(doc: &Document) -> Vec<ItemRef> {
    doc.main_view
        .item_placements
        .iter()
        .map(|(&key, _)| key)
        .collect()
}

#[test]
fn dirties_document_splits_edits_from_navigation() {
    use crate::core::document::TabRef;
    use crate::core::document::dock::{DockDrop, SplitSide};
    use scenarium::GraphId;

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
            to: BTreeSet::from([ItemRef::Node(NodeId::unique())]),
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
            tab: TabRef::Preferences,
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
            grabbed: ItemRef::Node(NodeId::unique()),
            moves: vec![(
                ItemRef::Node(NodeId::unique()),
                Vec2::ZERO,
                Vec2::new(5.0, 5.0),
            )],
        }),
        UndoStep::Doc(DocStep::RenameGraph {
            id: GraphId::unique(),
            from: "s".into(),
            to: "t".into(),
        }),
    ];
    for step in &content {
        assert!(step.dirties_document(), "content step must dirty: {step:?}",);
    }
}

#[test]
fn invalid_viewports_are_dropped_before_mutation() {
    let mut doc = Document::default();
    let initial = doc.main_view.viewport;
    let invalid = [
        Viewport {
            pan: Vec2::new(f32::NAN, 0.0),
            zoom: 1.0,
        },
        Viewport {
            pan: Vec2::new(0.0, f32::INFINITY),
            zoom: 1.0,
        },
        Viewport {
            pan: Vec2::ZERO,
            zoom: 0.0,
        },
        Viewport {
            pan: Vec2::ZERO,
            zoom: -1.0,
        },
        Viewport {
            pan: Vec2::ZERO,
            zoom: f32::NAN,
        },
        Viewport {
            pan: Vec2::ZERO,
            zoom: f32::INFINITY,
        },
        Viewport {
            pan: Vec2::ZERO,
            zoom: f32::NEG_INFINITY,
        },
    ];
    for to in invalid {
        assert!(
            commit_intent(Intent::SetViewport { to }, &mut doc, GraphRef::Main).is_err(),
            "invalid viewport {to:?} must be dropped"
        );
        assert_eq!(
            doc.main_view.viewport, initial,
            "an invalid viewport must not mutate the document"
        );
    }

    let valid = Viewport {
        pan: Vec2::new(10.0, -20.0),
        zoom: 2.0,
    };
    assert!(
        commit_intent(Intent::SetViewport { to: valid }, &mut doc, GraphRef::Main).is_ok(),
        "a finite positive viewport must commit"
    );
    assert_eq!(doc.main_view.viewport, valid);
    doc.validate().unwrap();
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
        .is_err(),
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
        .is_err(),
        "unsubscribing a missing edge is a no-op"
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
    doc.main_view.selected = node_ids.iter().copied().map(ItemRef::Node).collect();

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
    let new_ids: BTreeSet<ItemRef> = nodes
        .iter()
        .map(|(_, node_id, _)| ItemRef::Node(*node_id))
        .collect();
    assert!(
        new_ids.is_disjoint(&doc.main_view.selected),
        "clones get fresh ids"
    );
    let a_clone = nodes
        .iter()
        .find(|(pos, _, _)| *pos == Vec2::new(0.0, 0.0) + duplicate_offset())
        .map(|(_, node_id, _)| *node_id)
        .expect("a's clone offset from its origin");

    // Exactly two bindings survive: the internal a'->b' edge and the
    // Const; the external c->b edge (input 2) is gone.
    assert_eq!(bindings.len(), 2);
    let b_clone = nodes
        .iter()
        .find(|(pos, _, _)| *pos == Vec2::new(100.0, 0.0) + duplicate_offset())
        .map(|(_, node_id, _)| *node_id)
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
        .find(|(pos, _, _)| *pos == Vec2::new(100.0, 0.0) + duplicate_offset())
        .map(|(_, node_id, _)| *node_id)
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
    doc.main_view.selected = [ItemRef::Pin(OutputPort::new(id, 0))].into_iter().collect();
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
    doc.main_view.selected = [ItemRef::Node(a), ItemRef::Pin(OutputPort::new(b, 0))]
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
    let selected: BTreeSet<ItemRef> = [ItemRef::Node(node_id), ItemRef::Pin(port)]
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
        doc.graph.find(id, NodeSearch::TopLevel).unwrap().cache,
        CacheMode::None
    );
    assert!(!doc.graph.find(id, NodeSearch::TopLevel).unwrap().disabled);

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
        .unwrap_or_else(|_| panic!("{to:?} is a real change, not a no-op"));
        let node = doc.graph.find(id, NodeSearch::TopLevel).unwrap();
        match to {
            NodeProperty::RuntimeCache(m) => assert_eq!(node.cache, m),
            NodeProperty::Disabled(d) => assert_eq!(node.disabled, d),
        }
        assert!(
            !step.requires_relayout(),
            "a node-property toggle does not remeasure"
        );
        assert!(
            step.gesture_key().is_none(),
            "each toggle is its own undo entry"
        );
        revert_step(&step, &mut doc, GraphRef::Main);
        let node = doc.graph.find(id, NodeSearch::TopLevel).unwrap();
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
            .is_err(),
            "{to:?} equals the current value → writes nothing"
        );
    }
}

#[test]
fn set_output_pinned_commits_reverts_and_no_ops() {
    let mut doc = Document::default();
    let id = add_node_at(&mut doc, Vec2::ZERO);
    let port = OutputPort::new(id, 0);
    let key = ItemRef::Pin(port);
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
        pin_pos(&doc, port),
        Some(Vec2::ZERO),
        "pinning seeds an explicit zero-positioned item — no unset/sparse state"
    );
    assert_eq!(
        stack_order(&doc),
        vec![ItemRef::Node(id), key],
        "a fresh pin's item lands at the top of the paint stack"
    );
    assert!(!step.requires_relayout(), "a pin toggle does not remeasure");
    assert!(step.dirties_document(), "a real graph edit worth saving");
    assert!(
        step.gesture_key().is_none(),
        "each toggle is its own undo entry"
    );

    revert_step(&step, &mut doc, GraphRef::Main);
    assert!(!doc.graph.is_output_pinned(port), "revert clears it");
    assert_eq!(
        pin_pos(&doc, port),
        None,
        "revert removes the widget's item — no ghost slot in the stack"
    );
    apply_step(&step, &mut doc, GraphRef::Main);
    assert!(doc.graph.is_output_pinned(port), "redo re-marks it");

    // Bury the pin's widget at the *bottom* of the stack and give it a
    // real position, so the unpin→revert round-trip below has a
    // non-default slot and position to prove it restores.
    *doc.main_view.item_placements.get_mut(&key).unwrap() = Vec2::new(40.0, -30.0);
    doc.main_view.move_item_to_index(&key, 0);
    assert_eq!(stack_order(&doc), vec![key, ItemRef::Node(id)]);

    // Selecting the pin, then unpinning it, drops the selection — its
    // preview widget is gone; reverting the unpin restores it (mirrors
    // `RemoveNode`'s `selected`), along with the widget's exact position
    // and paint-stack slot.
    doc.main_view.selected.insert(key);
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
    assert_eq!(
        pin_pos(&doc, port),
        None,
        "unpinning removes the widget's item"
    );
    assert!(
        !doc.main_view.selected.contains(&key),
        "unpinning drops the now-gone widget's selection"
    );
    revert_step(&unpin, &mut doc, GraphRef::Main);
    assert!(doc.graph.is_output_pinned(port), "revert re-pins it");
    assert!(
        doc.main_view.selected.contains(&key),
        "revert restores the selection the pin had before it was unpinned"
    );
    assert_eq!(
        pin_pos(&doc, port),
        Some(Vec2::new(40.0, -30.0)),
        "revert restores the widget's exact position"
    );
    assert_eq!(
        stack_order(&doc),
        vec![key, ItemRef::Node(id)],
        "revert restores the widget's exact paint-stack slot (bottom), not the top"
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
        .is_err(),
        "already bound → writes nothing"
    );
}

/// A lone-pin `MoveSelection` intent: `grabbed`/`moves` target `port`,
/// no nodes in the group.
fn move_pin(port: OutputPort, to: Vec2) -> Intent {
    let key = ItemRef::Pin(port);
    Intent::MoveSelection {
        grabbed: key,
        moves: vec![(key, to)],
    }
}

#[test]
fn move_selection_repositions_a_pin_commits_reverts_and_coalesces() {
    let mut doc = Document::default();
    let id = add_node_at(&mut doc, Vec2::ZERO);
    let port = OutputPort::new(id, 0);

    // Pinning seeds a zero-default position — every pinned port has an
    // explicit item from the moment it's pinned, no unset/sparse state.
    commit_intent(
        Intent::SetOutputPinned {
            output: port,
            pinned: true,
        },
        &mut doc,
        GraphRef::Main,
    )
    .expect("pinning is a real change");
    assert_eq!(pin_pos(&doc, port), Some(Vec2::ZERO));

    let step = commit_intent(
        move_pin(port, Vec2::new(30.0, -12.0)),
        &mut doc,
        GraphRef::Main,
    )
    .expect("first drag off the seeded default is a real change");
    assert_eq!(pin_pos(&doc, port), Some(Vec2::new(30.0, -12.0)));
    assert!(
        !step.requires_relayout(),
        "repositioning a decoration (no nodes in the group) does not remeasure"
    );
    assert!(step.dirties_document(), "a real, persisted edit");
    assert_eq!(
        step.gesture_key(),
        Some(GestureKey::SelectionDrag(ItemRef::Pin(port))),
        "consecutive frames of the same pin's drag must coalesce"
    );

    // A later frame of the same drag: coalesce keeps the original
    // `from` (the seeded zero default) and adopts the new `to`.
    let step2 = build_step(move_pin(port, Vec2::new(50.0, -20.0)), &doc, GraphRef::Main).unwrap();
    apply_step(&step2, &mut doc, GraphRef::Main);
    let merged = step.coalesce(&step2).expect("same pin ⇒ coalesces");
    assert_eq!(
        merged.gesture_key(),
        Some(GestureKey::SelectionDrag(ItemRef::Pin(port))),
        "merged step keeps the same key"
    );

    // Reverting the *merged* step restores the original seeded (zero)
    // position rather than the drag's intermediate or final position.
    revert_step(&merged, &mut doc, GraphRef::Main);
    assert_eq!(
        pin_pos(&doc, port),
        Some(Vec2::ZERO),
        "revert restores the pre-drag default, not a leftover offset"
    );

    // Dragging to the exact position it already holds is a no-op.
    *doc.main_view
        .item_placements
        .get_mut(&ItemRef::Pin(port))
        .unwrap() = Vec2::new(1.0, 2.0);
    assert!(
        commit_intent(
            move_pin(port, Vec2::new(1.0, 2.0)),
            &mut doc,
            GraphRef::Main
        )
        .is_err(),
        "same position → writes nothing"
    );
}

#[test]
fn removing_a_node_captures_and_restores_its_pins() {
    // b's pin item is deliberately *interleaved* between the two node
    // items (stack: [a, pin, b]), so the undo has to restore not just the
    // pin's existence + position but its exact slot among survivors.
    let mut doc = Document::default();
    let a = add_node_at(&mut doc, Vec2::ZERO);
    let b = add_node_at(&mut doc, Vec2::new(100.0, 0.0));
    let port = OutputPort::new(b, 0);
    let key = ItemRef::Pin(port);
    commit_intent(
        Intent::SetOutputPinned {
            output: port,
            pinned: true,
        },
        &mut doc,
        GraphRef::Main,
    )
    .expect("pinning is a real change");
    *doc.main_view.item_placements.get_mut(&key).unwrap() = Vec2::new(7.0, 8.0);
    doc.main_view.move_item_to_index(&key, 1);
    doc.main_view.selected.insert(key);
    let expected = vec![ItemRef::Node(a), key, ItemRef::Node(b)];
    assert_eq!(stack_order(&doc), expected);

    let step = commit_intent(Intent::RemoveNode { node_id: b }, &mut doc, GraphRef::Main)
        .expect("removing an existing node is a real change");
    assert_eq!(
        stack_order(&doc),
        vec![ItemRef::Node(a)],
        "the node's own item and its pin's item are pruned together"
    );
    assert!(
        !doc.graph.is_output_pinned(port) && !doc.main_view.selected.contains(&key),
        "the pinned flag and selection membership go with the node"
    );

    revert_step(&step, &mut doc, GraphRef::Main);
    assert!(
        doc.graph.is_output_pinned(port),
        "undo re-pins the restored node's output"
    );
    assert_eq!(
        pin_pos(&doc, port),
        Some(Vec2::new(7.0, 8.0)),
        "undo restores the pin's custom position"
    );
    assert_eq!(
        stack_order(&doc),
        expected,
        "undo restores the exact interleaved paint-stack order"
    );
    assert!(
        doc.main_view.selected.contains(&key),
        "undo restores the pin's selection membership"
    );
    doc.validate().unwrap();
}

#[test]
fn raise_reorders_persists_and_undoes_for_nodes_and_pins() {
    let mut doc = Document::default();
    let a = add_node_at(&mut doc, Vec2::ZERO);
    let b = add_node_at(&mut doc, Vec2::new(100.0, 0.0));
    let c = add_node_at(&mut doc, Vec2::new(0.0, 100.0));
    let (a, b, c) = (ItemRef::Node(a), ItemRef::Node(b), ItemRef::Node(c));
    assert_eq!(
        stack_order(&doc),
        vec![a, b, c],
        "seed order is insertion order"
    );

    // Raise `a` (the back node) to the top — the end of `item_placements`,
    // painted last and so drawn in front.
    let step = commit_intent(Intent::Raise { key: a }, &mut doc, GraphRef::Main)
        .expect("raising a back node is a real reorder");
    assert_eq!(
        stack_order(&doc),
        vec![b, c, a],
        "a moved to the top of the stack"
    );

    // Stacking is view-state: undoable + persisted, but not dirty-worthy,
    // and it neither remeasures nor reshapes a graph interface.
    assert!(
        !step.dirties_document(),
        "a bare restack shouldn't nag on save"
    );
    assert!(!step.requires_relayout());
    assert!(
        step.gesture_key().is_none(),
        "each raise is its own undo entry"
    );

    // Undo restores the prior order; redo re-raises.
    revert_step(&step, &mut doc, GraphRef::Main);
    assert_eq!(
        stack_order(&doc),
        vec![a, b, c],
        "undo restores the prior order"
    );
    apply_step(&step, &mut doc, GraphRef::Main);
    assert_eq!(stack_order(&doc), vec![b, c, a], "redo re-raises a");

    // Raising the node already on top writes nothing.
    assert!(
        commit_intent(Intent::Raise { key: a }, &mut doc, GraphRef::Main).is_err(),
        "raising the frontmost item is a no-op"
    );

    // A pin's preview shares the same stack: pinning lands its item on
    // top; raising a node buries it; raising the pin lifts it back —
    // fully independent of its owner node's own slot (b stays put).
    let ItemRef::Node(b_id) = b else {
        unreachable!()
    };
    let port = OutputPort::new(b_id, 0);
    let pin = ItemRef::Pin(port);
    commit_intent(
        Intent::SetOutputPinned {
            output: port,
            pinned: true,
        },
        &mut doc,
        GraphRef::Main,
    )
    .expect("pinning is a real change");
    assert_eq!(stack_order(&doc), vec![b, c, a, pin]);
    commit_intent(Intent::Raise { key: c }, &mut doc, GraphRef::Main).expect("real reorder");
    assert_eq!(
        stack_order(&doc),
        vec![b, a, pin, c],
        "node c covers the pin"
    );
    let raise_pin = commit_intent(Intent::Raise { key: pin }, &mut doc, GraphRef::Main)
        .expect("raising a buried pin is a real reorder");
    assert_eq!(
        stack_order(&doc),
        vec![b, a, c, pin],
        "the pin lifts above every node; its owner b stays at the back"
    );
    revert_step(&raise_pin, &mut doc, GraphRef::Main);
    assert_eq!(stack_order(&doc), vec![b, a, pin, c], "pin raise undoes");
    apply_step(&raise_pin, &mut doc, GraphRef::Main);

    // The whole point: the mixed render order round-trips through save/load.
    let bytes = serde_json::to_vec_pretty(&doc).unwrap();
    let reloaded: Document = serde_json::from_slice(&bytes).unwrap();
    reloaded.validate().expect("reloaded document is valid");
    assert_eq!(
        stack_order(&reloaded),
        vec![b, a, c, pin],
        "the interleaved render order survives save/load"
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
                to: Some(Binding::bind(b, 0)),
            },
            &mut doc,
            GraphRef::Main,
        )
        .is_err(),
        "a bind that closes a cycle is rejected"
    );
    assert_eq!(
        doc.graph.bindings.get(&InputPort::new(a, 0)),
        None,
        "the rejected bind left a's input unbound"
    );
    assert_eq!(
        doc.graph.bindings.get(&InputPort::new(b, 0)),
        Some(&Binding::bind(a, 0)),
        "the existing a → b edge is untouched"
    );

    // A bind that keeps the graph acyclic still commits: c's input ← b's
    // output extends the chain into a → b → c.
    assert!(
        commit_intent(
            Intent::SetInput {
                input: InputPort::new(c, 0),
                to: Some(Binding::bind(b, 0)),
            },
            &mut doc,
            GraphRef::Main,
        )
        .is_ok(),
        "an acyclic bind commits"
    );
    assert_eq!(
        doc.graph.bindings.get(&InputPort::new(c, 0)),
        Some(&Binding::bind(b, 0)),
    );
}

/// A document holding an empty local graph "S" plus the interior view, so
/// `Local`-target intents resolve a scope.
fn doc_with_local_graph() -> (Document, GraphRef, GraphId) {
    let mut doc = Document::default();
    let id = GraphId::unique();
    doc.graph.insert_graph(id, GraphDef::new("S"));
    assert!(doc.ensure_sub_view(id));
    (doc, GraphRef::Local(id), id)
}

/// Commit `intent` expecting an `Invalid` refusal, then prove the document
/// is both unchanged and still structurally valid — a refusal that already
/// wrote half of itself would defeat the point.
#[track_caller]
fn assert_invalid(doc: &mut Document, target: GraphRef, intent: Intent, what: &str) {
    let nodes = doc.scope(target).expect("target resolves").graph.len();
    match commit_intent(intent, doc, target) {
        Err(Refusal::Invalid(_)) => {}
        other => panic!("{what}: expected an Invalid refusal, got {other:?}"),
    }
    assert_eq!(
        doc.scope(target).expect("target resolves").graph.len(),
        nodes,
        "{what}: a refused intent must not mutate the graph"
    );
    doc.validate()
        .unwrap_or_else(|e| panic!("{what}: document invalid after a refusal: {e}"));
}

/// Commit `intent` expecting a quiet refusal — the drop widgets rely on.
#[track_caller]
fn assert_quiet(doc: &mut Document, target: GraphRef, intent: Intent, what: &str) {
    match commit_intent(intent, doc, target) {
        Err(Refusal::Quiet) => {}
        other => panic!("{what}: expected a quiet refusal, got {other:?}"),
    }
}

fn func_node() -> Node {
    Node::new(NodeKind::Func(FuncId::unique()))
}

fn add_node(pos: Vec2, node_id: NodeId, node: Node) -> Intent {
    Intent::AddNode {
        pos,
        node_id,
        node,
        graph: None,
        bindings: vec![],
    }
}

#[test]
fn insertions_reusing_an_identity_are_refused_instead_of_panicking() {
    // Both of these used to abort the process on a payload a script can
    // hand `apply()`: `AddNode` tripped the absence assert in `apply_graph`,
    // `DuplicateNodes` tripped `Graph::insert`'s own duplicate-id panic.
    let mut doc = Document::default();
    let live = add_node_at(&mut doc, Vec2::ZERO);
    let repeated = NodeId::unique();

    let cases = [
        (
            "AddNode over a live id",
            add_node(Vec2::ZERO, live, func_node()),
        ),
        (
            "AddNode with a nil id",
            add_node(Vec2::ZERO, NodeId::nil(), func_node()),
        ),
        (
            "DuplicateNodes over a live id",
            Intent::DuplicateNodes {
                nodes: vec![(Vec2::ZERO, live, func_node())],
                bindings: vec![],
                subscriptions: vec![],
            },
        ),
        (
            "DuplicateNodes repeating one id within the batch",
            Intent::DuplicateNodes {
                nodes: vec![
                    (Vec2::ZERO, repeated, func_node()),
                    (Vec2::ONE, repeated, func_node()),
                ],
                bindings: vec![],
                subscriptions: vec![],
            },
        ),
    ];
    for (what, intent) in cases {
        assert_invalid(&mut doc, GraphRef::Main, intent, what);
    }

    // A node id that's live *inside a nested graph* collides just as hard:
    // scenarium requires ids to be unique across the whole authoring tree,
    // so a top-level-only check would let this through to `Document::validate`.
    let nested_id = GraphId::unique();
    let mut nested = GraphDef::new("S");
    let buried = nested.body.add(func_node());
    doc.graph.insert_graph(nested_id, nested);
    let instance = doc
        .graph
        .add(Node::new(NodeKind::Graph(GraphLink::Local(nested_id))));
    doc.main_view
        .item_placements
        .insert(ItemRef::Node(instance), Vec2::ZERO);
    assert_invalid(
        &mut doc,
        GraphRef::Main,
        add_node(Vec2::ZERO, buried, func_node()),
        "AddNode over an id buried in a nested graph",
    );
}

#[test]
fn malformed_payloads_are_refused_before_they_can_invalidate_the_document() {
    // Every case here would apply cleanly and leave a document that
    // `Document::validate` rejects — which, because saving only validates
    // in debug builds, means a project that writes fine and won't reopen.
    let mut doc = Document::default();
    let live = add_node_at(&mut doc, Vec2::ZERO);
    let ghost = NodeId::unique();
    let nan = Vec2::new(f32::NAN, 0.0);

    let mut nil_func = func_node();
    nil_func.kind = NodeKind::Func(FuncId::nil());
    let mut dangling_link = func_node();
    dangling_link.kind = NodeKind::Graph(GraphLink::Local(GraphId::unique()));

    let cases = [
        (
            "AddNode at a non-finite position",
            add_node(nan, NodeId::unique(), func_node()),
        ),
        (
            "AddNode with a nil func id",
            add_node(Vec2::ZERO, NodeId::unique(), nil_func),
        ),
        (
            "AddNode linking a local graph the target doesn't hold",
            add_node(Vec2::ZERO, NodeId::unique(), dangling_link),
        ),
        (
            "AddNode seeding a binding from a producer that isn't there",
            Intent::AddNode {
                pos: Vec2::ZERO,
                node_id: NodeId::unique(),
                node: func_node(),
                graph: None,
                bindings: vec![(InputPort::new(ghost, 0), Binding::bind(live, 0))],
            },
        ),
        (
            "DuplicateNodes subscribing a node that isn't there",
            Intent::DuplicateNodes {
                nodes: vec![(Vec2::ZERO, NodeId::unique(), func_node())],
                bindings: vec![],
                subscriptions: vec![Subscription {
                    emitter: ghost,
                    event_idx: 0,
                    subscriber: live,
                }],
            },
        ),
        (
            "MoveSelection to a non-finite position",
            Intent::MoveSelection {
                grabbed: ItemRef::Node(live),
                moves: vec![(ItemRef::Node(live), nan)],
            },
        ),
        (
            "SetSubscription carrying a nil id",
            Intent::SetSubscription {
                emitter: NodeId::nil(),
                event_idx: 0,
                subscriber: live,
                subscribe: true,
            },
        ),
    ];
    for (what, intent) in cases {
        assert_invalid(&mut doc, GraphRef::Main, intent, what);
    }

    // A definition arriving with a new node must not reuse a graph id the
    // document already holds — `Graph::validate` rejects a duplicate.
    let taken = GraphId::unique();
    doc.graph.insert_graph(taken, GraphDef::new("S"));
    let mut instance = func_node();
    instance.kind = NodeKind::Graph(GraphLink::Local(taken));
    assert_invalid(
        &mut doc,
        GraphRef::Main,
        Intent::AddNode {
            pos: Vec2::ZERO,
            node_id: NodeId::unique(),
            node: instance,
            graph: Some((taken, Box::new(GraphDef::new("clash")))),
            bindings: vec![],
        },
        "AddNode bringing a definition under an id already in use",
    );

    // The entry graph has no interface, so a boundary node there is
    // `DocumentValidationError::EntryBoundaryNodes` on the next load.
    assert_invalid(
        &mut doc,
        GraphRef::Main,
        add_node(
            Vec2::ZERO,
            NodeId::unique(),
            Node::new(NodeKind::GraphInput),
        ),
        "AddNode putting a boundary node in the entry graph",
    );

    // A graph interior accepts one boundary node per side, never two.
    let (mut doc, target, _) = doc_with_local_graph();
    assert!(
        commit_intent(
            add_node(
                Vec2::ZERO,
                NodeId::unique(),
                Node::new(NodeKind::GraphInput)
            ),
            &mut doc,
            target,
        )
        .is_ok(),
        "the interior's first boundary input commits"
    );
    assert_invalid(
        &mut doc,
        target,
        add_node(
            Vec2::ZERO,
            NodeId::unique(),
            Node::new(NodeKind::GraphInput),
        ),
        "AddNode adding a second boundary input",
    );
}

#[test]
fn stale_references_still_refuse_quietly() {
    // Widgets emit identities they read out of the live document, so the
    // only thing they get wrong is staleness — an anchor removed between
    // the gesture starting and the intent draining. Those stay silent;
    // turning them into reported failures would spam the status bar on
    // ordinary use.
    let mut doc = Document::default();
    let live = add_node_at(&mut doc, Vec2::ZERO);
    let gone = NodeId::unique();

    let cases = [
        ("RemoveNode", Intent::RemoveNode { node_id: gone }),
        (
            "RenameNode",
            Intent::RenameNode {
                node_id: gone,
                to: "x".into(),
            },
        ),
        (
            "SetNodeProperty",
            Intent::SetNodeProperty {
                node_id: gone,
                to: NodeProperty::Disabled(true),
            },
        ),
        (
            "SetOutputPinned",
            Intent::SetOutputPinned {
                output: OutputPort::new(gone, 0),
                pinned: true,
            },
        ),
        ("DetachGraph", Intent::DetachGraph { node_id: gone }),
        (
            "Raise",
            Intent::Raise {
                key: ItemRef::Node(gone),
            },
        ),
        (
            "SetInput onto a vanished node",
            Intent::SetInput {
                input: InputPort::new(gone, 0),
                to: None,
            },
        ),
        (
            // The held-wire case: the producer was removed after the drag
            // began, so committing would leave a dangling edge.
            "SetInput from a vanished producer",
            Intent::SetInput {
                input: InputPort::new(live, 0),
                to: Some(Binding::bind(gone, 0)),
            },
        ),
        (
            // An event wire dropped on a node that's since gone: dropped
            // rather than recorded as a dangling subscription.
            "SetSubscription onto a vanished subscriber",
            Intent::SetSubscription {
                emitter: live,
                event_idx: 0,
                subscriber: gone,
                subscribe: true,
            },
        ),
        (
            // A satellite drag outliving its anchor: every member is
            // filtered out, and the empty batch is a no-op, not an error.
            "MoveSelection of a pin whose node vanished",
            move_pin(OutputPort::new(gone, 0), Vec2::ZERO),
        ),
    ];
    for (what, intent) in cases {
        assert_quiet(&mut doc, GraphRef::Main, intent, what);
    }
    assert!(doc.graph.bindings.is_empty(), "nothing was written");
    doc.validate().expect("document stays valid");
}

#[test]
fn selection_and_move_drop_members_whose_widget_is_gone() {
    // The rubber band and a group drag both snapshot identities when the
    // gesture starts, and undo runs before the gesture prepass — so a
    // member can disappear mid-gesture. Recording it verbatim would leave
    // `selected` naming an item the view can't render, which is exactly
    // `GraphViewValidationError::MissingSelectedItem`.
    let mut doc = Document::default();
    let live = add_node_at(&mut doc, Vec2::ZERO);
    let gone = NodeId::unique();

    let step = commit_intent(
        Intent::SetSelection {
            to: [ItemRef::Node(live), ItemRef::Node(gone)]
                .into_iter()
                .collect(),
        },
        &mut doc,
        GraphRef::Main,
    )
    .expect("a selection with one live member commits");
    let UndoStep::Graph(GraphStep::SetSelection { to, .. }) = &step else {
        panic!("expected a SetSelection step, got {step:?}");
    };
    assert_eq!(
        to,
        &[ItemRef::Node(live)].into_iter().collect::<BTreeSet<_>>(),
        "the vanished member is dropped, the live one kept"
    );
    assert_eq!(doc.main_view.selected, *to);

    let step = commit_intent(
        Intent::MoveSelection {
            grabbed: ItemRef::Node(live),
            moves: vec![
                (ItemRef::Node(live), Vec2::new(5.0, 6.0)),
                (ItemRef::Node(gone), Vec2::new(7.0, 8.0)),
            ],
        },
        &mut doc,
        GraphRef::Main,
    )
    .expect("a move with one live member commits");
    let UndoStep::Graph(GraphStep::MoveSelection { moves, .. }) = &step else {
        panic!("expected a MoveSelection step, got {step:?}");
    };
    assert_eq!(
        moves,
        &[(ItemRef::Node(live), Vec2::ZERO, Vec2::new(5.0, 6.0))],
        "only the surviving member is recorded"
    );
    doc.validate().expect("document stays valid");
}

#[test]
fn duplicating_a_selection_skips_the_boundary_node() {
    // A graph holds at most one boundary node per side, so copying one
    // would make the interior invalid. Ctrl+D over a selection that
    // includes it duplicates the rest.
    let mut doc = Document::default();
    let id = GraphId::unique();
    let mut def = GraphDef::new("S");
    let boundary = def.body.add(Node::new(NodeKind::GraphInput));
    let func = def.body.add(func_node());
    doc.graph.insert_graph(id, def);
    assert!(doc.ensure_sub_view(id));
    let target = GraphRef::Local(id);

    let selection: BTreeSet<NodeId> = [boundary, func].into_iter().collect();
    let intent = build_duplicate_intent_for(&doc, target, &selection, false)
        .expect("the func node is duplicable");
    let Intent::DuplicateNodes { nodes, .. } = &intent else {
        panic!("expected DuplicateNodes, got {intent:?}");
    };
    assert_eq!(nodes.len(), 1, "only the func node is cloned");
    assert!(
        !nodes.iter().any(|(_, id, _)| *id == boundary),
        "the clone gets a fresh id and is never the boundary node"
    );

    commit_intent(intent, &mut doc, target).expect("the duplicate commits");
    doc.validate()
        .expect("the interior keeps exactly one boundary input");

    // A selection of nothing but the boundary node yields no intent at all,
    // rather than an empty batch that would just clear the selection.
    let only_boundary: BTreeSet<NodeId> = [boundary].into_iter().collect();
    assert!(
        build_duplicate_intent_for(&doc, target, &only_boundary, false).is_none(),
        "nothing duplicable means no intent"
    );
}
