use std::collections::BTreeSet;

use glam::Vec2;
use scenarium::StaticValue;
use scenarium::{Binding, CacheMode, InputPort, Node, NodeId, NodeKind, NodeSearch};
use scenarium::{DataType, FuncId, FuncInput};
use scenarium::{GraphDef, GraphId, GraphLink, Subscription};

use crate::core::document::dock::DockOp;
use crate::core::document::{Document, GraphRef, Viewport};
use crate::core::edit::intent::apply::{apply_step, commit_doc_intent, commit_intent, revert_step};
use crate::core::edit::intent::build::{build_doc_step, build_step};
use crate::core::edit::intent::duplicate::internals::duplicate_offset;
use crate::core::edit::intent::duplicate::{build_duplicate_intent, build_duplicate_intent_for};
use crate::core::edit::intent::types::{
    BatchScope, DocIntent, DocStep, GraphStep, Intent, NodeProperty, Refusal, UndoStep,
};

/// Add a bare `Func`-kind node to `doc`'s root graph + main view at
/// `pos`, returning its id.
fn add_node_at(doc: &mut Document, pos: Vec2) -> NodeId {
    let node = Node::new(NodeKind::Func(FuncId::unique()));
    let id = doc.graph.add(node);
    doc.main_view.item_placements.insert(id, pos);
    id
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
    let dock_step = |op: DockOp| build_doc_step(DocIntent::Dock(op), &dock_doc).map(UndoStep::Doc);

    // Navigation-only steps: camera, selection, tab focus — the user
    // doesn't "save" these, so they must not flip the unsaved flag.
    let navigation = [
        UndoStep::Graph(GraphStep::SetSelection {
            from: BTreeSet::new(),
            to: BTreeSet::from([NodeId::unique()]),
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
            grabbed: NodeId::unique(),
            moves: vec![(NodeId::unique(), Vec2::ZERO, Vec2::new(5.0, 5.0))],
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

/// A true arm here costs a whole extra record pass, and the two steps most
/// at risk — a node drag and a divider drag — emit one per *gesture frame*,
/// so a spurious true doubles the editor pipeline for the length of the
/// drag. The split under test: only a step that changes a widget's measured
/// size, or introduces a node with no cached port offsets, may return true.
#[test]
fn invalidates_cached_geometry_splits_resizes_from_moves() {
    use crate::core::document::TabRef;
    use crate::core::document::dock::{DockDrop, DockPath, SplitSide};
    use scenarium::{GraphId, StaticValue};

    let mut dock_doc = Document::default();
    let primary = dock_doc.layout.primary().id;
    dock_doc.layout.find_or_insert(TabRef::Preferences, primary);
    // Split Preferences into its own pane, then keep the step: it is both a
    // structural dock op for the table below and what gives `SetRatio` a
    // real divider to name instead of a refused no-op.
    let split = commit_doc_intent(
        DocIntent::Dock(DockOp::MoveTab {
            tab: TabRef::Preferences,
            to: DockDrop::Split {
                group: primary,
                side: SplitSide::Right,
            },
        }),
        &mut dock_doc,
    )
    .expect("splitting a second tab off the primary group");
    let dock_step = |op: DockOp| {
        UndoStep::Doc(build_doc_step(DocIntent::Dock(op), &dock_doc).expect("a real dock op"))
    };
    let node_id = NodeId::unique();
    let port = InputPort::new(node_id, 0);
    let cst = |v: f64| Some(Binding::Const(StaticValue::Float(v)));

    // Nothing remeasures: a port center is `node.pos + cached offset`, and
    // every one of these leaves that offset valid.
    let moves = [
        // The node drag. Emits one step per gesture frame, drains
        // pre-record, and Pass A already arranges at the cursor.
        UndoStep::Graph(GraphStep::MoveSelection {
            grabbed: node_id,
            moves: vec![(node_id, Vec2::ZERO, Vec2::new(5.0, 5.0))],
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
        UndoStep::Graph(GraphStep::SetSelection {
            from: BTreeSet::new(),
            to: BTreeSet::from([node_id]),
        }),
        // The divider drag: `Splitter` lays out at the live pointer ratio,
        // so Pass A already drew what this step persists.
        dock_step(DockOp::SetRatio {
            split: DockPath::ROOT,
            ratio: 0.7,
        }),
        // Panes reshape; no node's content does.
        split,
        // Focus back to the other pane — Preferences is the focused one
        // after the split, so activating it again would be a no-op.
        dock_step(DockOp::ActivateTab {
            tab: TabRef::Graph(GraphRef::Main),
        }),
        // Value-only: the editor stays present at its `Fixed` size.
        UndoStep::Graph(GraphStep::SetInput {
            input: port,
            from: cst(1.0),
            to: cst(2.0),
        }),
    ];
    for step in &moves {
        assert!(
            !step.is_noop(),
            "a degenerate step would pin nothing: {step:?}"
        );
        assert!(
            !step.invalidates_cached_geometry(),
            "a move must not cost a second record pass: {step:?}",
        );
    }

    // Each of these changes a measured size, or brings in a node that has
    // never recorded — so the cached offsets wires anchor to are stale.
    let resizes = [
        UndoStep::Graph(GraphStep::RenameNode {
            node_id,
            from: "a".into(),
            to: "a-much-longer-title".into(),
        }),
        // Adding the inline const editor resizes the node and shifts every
        // port row below it.
        UndoStep::Graph(GraphStep::SetInput {
            input: port,
            from: None,
            to: cst(1.0),
        }),
        // ...and removing it is the connection commit, the case Pass B has
        // always existed for.
        UndoStep::Graph(GraphStep::SetInput {
            input: port,
            from: cst(1.0),
            to: None,
        }),
        UndoStep::Doc(DocStep::RenameGraph {
            id: GraphId::unique(),
            from: "s".into(),
            to: "t".into(),
        }),
    ];
    for step in &resizes {
        assert!(
            !step.is_noop(),
            "a degenerate step would pin nothing: {step:?}"
        );
        assert!(
            step.invalidates_cached_geometry(),
            "a resize strands the offset cache: {step:?}",
        );
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
    revert_step(&step, &mut doc, BatchScope::Graph(GraphRef::Main));
    assert!(!doc.graph.is_subscribed(emitter, 0, subscriber));
    apply_step(&step, &mut doc, BatchScope::Graph(GraphRef::Main));
    assert!(doc.graph.is_subscribed(emitter, 0, subscriber));

    // Unsubscribe commits, removes the edge, and undo brings it back.
    let step = commit_intent(
        set_sub(emitter, 0, subscriber, false),
        &mut doc,
        GraphRef::Main,
    )
    .expect("unsubscribe commits");
    assert!(!doc.graph.is_subscribed(emitter, 0, subscriber));
    revert_step(&step, &mut doc, BatchScope::Graph(GraphRef::Main));
    assert!(doc.graph.is_subscribed(emitter, 0, subscriber));

    // Redo the unsubscribe (apply writes the `to = unsubscribed` half),
    // then unsubscribing the now-absent edge is a no-op.
    apply_step(&step, &mut doc, BatchScope::Graph(GraphRef::Main));
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
    doc.main_view.selected = node_ids.iter().copied().collect();

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
    let new_ids: BTreeSet<NodeId> = nodes.iter().map(|(_, node_id, _)| *node_id).collect();
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
            !step.invalidates_cached_geometry(),
            "a node-property toggle does not remeasure"
        );
        assert!(
            step.gesture_key().is_none(),
            "each toggle is its own undo entry"
        );
        revert_step(&step, &mut doc, BatchScope::Graph(GraphRef::Main));
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
fn requires_reconcile_splits_retained_set_movers_from_the_rest() {
    use crate::core::document::{BoundarySide, TabRef};
    use scenarium::DataType;

    let mut doc = Document::default();
    let node = add_node_at(&mut doc, Vec2::ZERO);
    let primary = doc.layout.primary().id;
    doc.layout
        .find_or_insert(TabRef::ImageViewer(node), primary);
    let def_id = GraphId::unique();

    // Steps that can move the set of live preview nodes, so the store has to
    // re-derive it and release or upload accordingly.
    let movers = [
        // Any step that adds or removes nodes can add or remove a preview.
        build_step(Intent::RemoveNode { node_id: node }, &doc, GraphRef::Main)
            .expect("removing a live node builds"),
        UndoStep::Graph(GraphStep::AddNode {
            pos: Vec2::ZERO,
            node_id: NodeId::unique(),
            node: func_node(),
            graph: None,
            bindings: Vec::new(),
        }),
        UndoStep::Graph(GraphStep::DuplicateNodes {
            nodes: vec![(Vec2::ZERO, NodeId::unique(), func_node())],
            bindings: Vec::new(),
            subscriptions: Vec::new(),
            from_selection: BTreeSet::new(),
            to_selection: BTreeSet::from([node]),
        }),
        // Any dock op is a whole-layout swap, so it can open, close, or
        // relocate a viewer tab.
        UndoStep::Doc(
            build_doc_step(
                DocIntent::Dock(DockOp::CloseTab {
                    tab: TabRef::ImageViewer(node),
                }),
                &doc,
            )
            .expect("closing an open viewer tab builds"),
        ),
    ];
    for step in &movers {
        assert!(
            step.requires_reconcile(),
            "step can move the retained set: {step:?}",
        );
    }

    // Everything else leaves the set exactly as it was — a preview is
    // entry-only, so forking a definition cannot carry one along either.
    let others = [
        UndoStep::Graph(GraphStep::DetachGraph {
            node_id: node,
            from_id: def_id,
            to_id: GraphId::unique(),
            graph: Box::new(GraphDef::new("fork")),
        }),
        UndoStep::Graph(GraphStep::MoveSelection {
            grabbed: node,
            moves: vec![(node, Vec2::ZERO, Vec2::new(9.0, 9.0))],
        }),
        UndoStep::Graph(GraphStep::Raise {
            key: node,
            from_index: 0,
            to_index: 1,
        }),
        UndoStep::Graph(GraphStep::RenameNode {
            node_id: node,
            from: "a".into(),
            to: "b".into(),
        }),
        UndoStep::Graph(GraphStep::SetInput {
            input: InputPort::new(node, 0),
            from: None,
            to: Some(Binding::Const(StaticValue::Int(1))),
        }),
        UndoStep::Graph(GraphStep::SetSelection {
            from: BTreeSet::new(),
            to: BTreeSet::from([node]),
        }),
        UndoStep::Graph(GraphStep::SetNodeProperty {
            node_id: node,
            from: NodeProperty::RuntimeCache(CacheMode::None),
            to: NodeProperty::RuntimeCache(CacheMode::Ram),
        }),
        UndoStep::Graph(GraphStep::SetSubscription {
            emitter: node,
            event_idx: 0,
            subscriber: NodeId::unique(),
            from: false,
            to: true,
        }),
        UndoStep::Graph(GraphStep::SetViewport {
            from: Viewport {
                pan: Vec2::ZERO,
                zoom: 1.0,
            },
            to: Viewport {
                pan: Vec2::splat(4.0),
                zoom: 2.0,
            },
        }),
        UndoStep::Doc(DocStep::RenameGraph {
            id: def_id,
            from: "s".into(),
            to: "t".into(),
        }),
        UndoStep::Doc(DocStep::AddBoundaryPort {
            graph_id: def_id,
            side: BoundarySide::Input,
            idx: 0,
            name: "input0".into(),
            data_type: DataType::Int,
        }),
    ];
    for step in &others {
        assert!(
            !step.requires_reconcile(),
            "step cannot move the retained set: {step:?}",
        );
    }
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
    doc.main_view.item_placements.insert(instance, Vec2::ZERO);
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
                grabbed: live,
                moves: vec![(live, nan)],
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
        (
            "AddLocalGraphInstance naming a graph the target doesn't hold",
            Intent::AddLocalGraphInstance {
                pos: Vec2::ZERO,
                node_id: NodeId::unique(),
                graph_id: GraphId::unique(),
            },
        ),
        (
            "AddLocalGraphInstance with a nil graph id",
            Intent::AddLocalGraphInstance {
                pos: Vec2::ZERO,
                node_id: NodeId::unique(),
                graph_id: GraphId::nil(),
            },
        ),
        (
            "AddLocalGraphInstance over a live node id",
            Intent::AddLocalGraphInstance {
                pos: Vec2::ZERO,
                node_id: live,
                graph_id: GraphId::unique(),
            },
        ),
        (
            "AddLocalGraphInstance at a non-finite position",
            Intent::AddLocalGraphInstance {
                pos: nan,
                node_id: NodeId::unique(),
                graph_id: GraphId::unique(),
            },
        ),
        (
            "AddLocalGraph with a nil graph id",
            Intent::AddLocalGraph {
                pos: Vec2::ZERO,
                node_id: NodeId::unique(),
                graph_id: GraphId::nil(),
                def: Box::new(GraphDef::new("nil")),
            },
        ),
        (
            "AddLocalGraph over a live node id",
            Intent::AddLocalGraph {
                pos: Vec2::ZERO,
                node_id: live,
                graph_id: GraphId::unique(),
                def: Box::new(GraphDef::new("collides")),
            },
        ),
        (
            "AddLocalGraph at a non-finite position",
            Intent::AddLocalGraph {
                pos: nan,
                node_id: NodeId::unique(),
                graph_id: GraphId::unique(),
                def: Box::new(GraphDef::new("nan")),
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
    assert_invalid(
        &mut doc,
        GraphRef::Main,
        Intent::AddLocalGraph {
            pos: Vec2::ZERO,
            node_id: NodeId::unique(),
            graph_id: taken,
            def: Box::new(GraphDef::new("clash")),
        },
        "AddLocalGraph bringing a definition under an id already in use",
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
fn instancing_a_local_graph_reads_its_definition_out_of_the_target() {
    // The palette hands over an id and nothing else, so this is where the
    // node's name and its interface's const defaults come from. Two ports:
    // one defaulted, one not, so the seeded bindings are a filter and not
    // "one per input".
    let mut doc = Document::default();
    let graph_id = GraphId::unique();
    doc.graph.insert_graph(
        graph_id,
        GraphDef::new("Blur")
            .input(FuncInput::optional("radius", DataType::Float).default(StaticValue::Float(2.5)))
            .input(FuncInput::required("image", DataType::Float)),
    );

    let node_id = NodeId::unique();
    let instance = |node_id| Intent::AddLocalGraphInstance {
        pos: Vec2::new(12.0, 34.0),
        node_id,
        graph_id,
    };
    commit_intent(instance(node_id), &mut doc, GraphRef::Main).expect("instancing commits");

    let node = doc
        .graph
        .find(node_id, NodeSearch::TopLevel)
        .expect("instance node added");
    assert_eq!(node.kind, NodeKind::Graph(GraphLink::Local(graph_id)));
    assert_eq!(node.name, "Blur", "the node is named after the definition");
    assert_eq!(
        doc.graph.graphs.len(),
        1,
        "the definition was already there — instancing copies nothing"
    );
    assert_eq!(
        doc.main_view.item_placements[&node_id],
        Vec2::new(12.0, 34.0)
    );
    assert_eq!(
        doc.graph.bindings.get(&InputPort::new(node_id, 0)),
        Some(&Binding::Const(StaticValue::Float(2.5))),
        "the defaulted interface input is seeded"
    );
    assert_eq!(
        doc.graph.bindings.get(&InputPort::new(node_id, 1)),
        None,
        "an input with no default stays unbound"
    );

    // A second instance shares the one definition rather than forking it.
    let second = NodeId::unique();
    commit_intent(instance(second), &mut doc, GraphRef::Main).expect("a second instance commits");
    assert_eq!(doc.graph.graphs.len(), 1);
    assert_eq!(
        doc.graph.find(second, NodeSearch::TopLevel).unwrap().kind,
        NodeKind::Graph(GraphLink::Local(graph_id))
    );
    doc.validate().expect("document valid after instancing");

    // A `Local` link resolves only against the graph that holds the
    // definition, so a sibling scope cannot instance it — the definition
    // above lives in root, and this target is a different graph's interior.
    let (mut nested_doc, nested_target, _) = doc_with_local_graph();
    let outsider = GraphId::unique();
    nested_doc
        .graph
        .insert_graph(outsider, GraphDef::new("Elsewhere"));
    assert_invalid(
        &mut nested_doc,
        nested_target,
        Intent::AddLocalGraphInstance {
            pos: Vec2::ZERO,
            node_id: NodeId::unique(),
            graph_id: outsider,
        },
        "AddLocalGraphInstance naming a definition held by another graph",
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
        ("DetachGraph", Intent::DetachGraph { node_id: gone }),
        ("Raise", Intent::Raise { key: gone }),
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
            // A drag outliving its target: every member is filtered out, and
            // the empty batch is a no-op, not an error.
            "MoveSelection of an item whose node vanished",
            Intent::MoveSelection {
                grabbed: gone,
                moves: vec![(gone, Vec2::ZERO)],
            },
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
            to: [live, gone].into_iter().collect(),
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
        &[live].into_iter().collect::<BTreeSet<_>>(),
        "the vanished member is dropped, the live one kept"
    );
    assert_eq!(doc.main_view.selected, *to);

    let step = commit_intent(
        Intent::MoveSelection {
            grabbed: live,
            moves: vec![(live, Vec2::new(5.0, 6.0)), (gone, Vec2::new(7.0, 8.0))],
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
        &[(live, Vec2::ZERO, Vec2::new(5.0, 6.0))],
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
