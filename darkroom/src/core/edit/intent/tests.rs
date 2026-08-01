use std::collections::BTreeSet;

use glam::Vec2;
use scenarium::ConstValue;
use scenarium::FuncId;
use scenarium::Subscription;
use scenarium::{Binding, CacheMode, InputPort, Node, NodeId, NodeKind};

use crate::core::document::harness::DocFixture;
use crate::core::document::{Document, Viewport};
use crate::core::edit::intent::apply::{apply_step, commit_intent, revert_step};
use crate::core::edit::intent::duplicate::build_duplicate_intent;
use crate::core::edit::intent::duplicate::internals::duplicate_offset;
use crate::core::edit::intent::types::{GraphIntent, NodeProperty, UndoStep};

#[test]
fn dirties_document_splits_edits_from_navigation() {
    // Navigation-only steps: camera, selection, tab focus — the user
    // doesn't "save" these, so they must not flip the unsaved flag.
    let navigation = [
        UndoStep::SetSelection {
            from: BTreeSet::new(),
            to: BTreeSet::from([NodeId::unique()]),
        },
        UndoStep::SetViewport {
            from: Viewport {
                pan: Vec2::ZERO,
                zoom: 1.0,
            },
            to: Viewport {
                pan: Vec2::new(10.0, 20.0),
                zoom: 2.0,
            },
        },
    ];
    for step in &navigation {
        assert!(
            !step.dirties_document(),
            "navigation step must not dirty: {step:?}",
        );
    }

    // Content steps: graph data + node layout — real, savable work.
    let content = [
        UndoStep::RenameNode {
            node_id: NodeId::unique(),
            from: "a".into(),
            to: "b".into(),
        },
        UndoStep::MoveSelection {
            grabbed: NodeId::unique(),
            moves: vec![(NodeId::unique(), Vec2::ZERO, Vec2::new(5.0, 5.0))],
        },
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
    use scenarium::ConstValue;

    let node_id = NodeId::unique();
    let port = InputPort::new(node_id, 0);
    let cst = |v: f64| Some(Binding::Const(ConstValue::Float(v)));

    // Nothing remeasures: a port center is `node.pos + cached offset`, and
    // every one of these leaves that offset valid.
    let moves = [
        // The node drag. Emits one step per gesture frame, drains
        // pre-record, and Pass A already arranges at the cursor.
        UndoStep::MoveSelection {
            grabbed: node_id,
            moves: vec![(node_id, Vec2::ZERO, Vec2::new(5.0, 5.0))],
        },
        UndoStep::SetViewport {
            from: Viewport {
                pan: Vec2::ZERO,
                zoom: 1.0,
            },
            to: Viewport {
                pan: Vec2::new(10.0, 20.0),
                zoom: 2.0,
            },
        },
        UndoStep::SetSelection {
            from: BTreeSet::new(),
            to: BTreeSet::from([node_id]),
        },
        // Value-only: the editor stays present at its `Fixed` size.
        UndoStep::SetInput {
            input: port,
            from: cst(1.0),
            to: cst(2.0),
        },
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
        UndoStep::RenameNode {
            node_id,
            from: "a".into(),
            to: "a-much-longer-title".into(),
        },
        // Adding the inline const editor resizes the node and shifts every
        // port row below it.
        UndoStep::SetInput {
            input: port,
            from: None,
            to: cst(1.0),
        },
        // ...and removing it is the connection commit, the case Pass B has
        // always existed for.
        UndoStep::SetInput {
            input: port,
            from: cst(1.0),
            to: None,
        },
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
            commit_intent(GraphIntent::SetViewport { to }, &mut doc).is_err(),
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
        commit_intent(GraphIntent::SetViewport { to: valid }, &mut doc)
            .unwrap()
            .is_some(),
        "a finite positive viewport must commit"
    );
    assert_eq!(doc.main_view.viewport, valid);
    doc.validate().unwrap();
}

#[test]
fn subscribe_unsubscribe_commit_and_undo() {
    let mut fixture = DocFixture::default();
    let emitter = fixture.stub_at(Vec2::ZERO);
    let subscriber = fixture.stub_at(Vec2::new(100.0, 0.0));
    let mut doc = fixture.doc;
    let set_sub = |e, i, s, subscribe| GraphIntent::SetSubscription {
        emitter: e,
        event_idx: i,
        subscriber: s,
        subscribe,
    };

    // Subscribe commits and writes the edge.
    let step = commit_intent(set_sub(emitter, 0, subscriber, true), &mut doc)
        .unwrap()
        .expect("subscribe commits");
    assert!(doc.graph.is_subscribed(emitter, 0, subscriber));

    // A second identical subscribe is a no-op (from == to == true).
    assert!(
        commit_intent(set_sub(emitter, 0, subscriber, true), &mut doc)
            .unwrap()
            .is_none(),
        "re-subscribing the same edge is a no-op"
    );

    // Undo removes it; redo restores it.
    revert_step(&step, &mut doc);
    assert!(!doc.graph.is_subscribed(emitter, 0, subscriber));
    apply_step(&step, &mut doc);
    assert!(doc.graph.is_subscribed(emitter, 0, subscriber));

    // Unsubscribe commits, removes the edge, and undo brings it back.
    let step = commit_intent(set_sub(emitter, 0, subscriber, false), &mut doc)
        .unwrap()
        .expect("unsubscribe commits");
    assert!(!doc.graph.is_subscribed(emitter, 0, subscriber));
    revert_step(&step, &mut doc);
    assert!(doc.graph.is_subscribed(emitter, 0, subscriber));

    // Redo the unsubscribe (apply writes the `to = unsubscribed` half),
    // then unsubscribing the now-absent edge is a no-op.
    apply_step(&step, &mut doc);
    assert!(!doc.graph.is_subscribed(emitter, 0, subscriber));
    assert!(
        commit_intent(set_sub(emitter, 0, subscriber, false), &mut doc)
            .unwrap()
            .is_none(),
        "unsubscribing a missing edge is a no-op"
    );
}

#[test]
fn duplicate_intent_drops_or_keeps_external_by_flag() {
    // a -> b (internal edge, both selected); c -> b (external, c not
    // selected). b also has a Const on input 1. Selecting {a, b} must
    // duplicate a' and b', keep a'->b' and the Const, drop c->b.
    let mut fixture = DocFixture::default();
    let a = fixture.stub_at(Vec2::new(0.0, 0.0));
    let b = fixture.stub_at(Vec2::new(100.0, 0.0));
    let c = fixture.stub_at(Vec2::new(0.0, 100.0));
    let mut doc = fixture.doc;
    doc.graph
        .set_input_binding(InputPort::new(b, 0), Binding::bind(a, 0));
    doc.graph
        .set_input_binding(InputPort::new(b, 1), Binding::Const(ConstValue::from(7i64)));
    doc.graph
        .set_input_binding(InputPort::new(b, 2), Binding::bind(c, 0));
    doc.main_view.selected = [a, b].into_iter().collect();

    let Some(GraphIntent::DuplicateNodes {
        nodes,
        bindings,
        subscriptions,
    }) = build_duplicate_intent(&doc, false)
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
    let Some(GraphIntent::DuplicateNodes {
        nodes: incoming_nodes,
        bindings: incoming,
        ..
    }) = build_duplicate_intent(&doc, true)
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
    let mut fixture = DocFixture::default();
    fixture.stub_at(Vec2::ZERO);
    assert!(build_duplicate_intent(&fixture.doc, false).is_none());
}

#[test]
fn set_node_property_commits_and_reverts() {
    let mut fixture = DocFixture::default();
    let id = fixture.stub_at(Vec2::ZERO);
    let mut doc = fixture.doc;
    // Fresh nodes default to no caching (None) and enabled.
    assert_eq!(doc.graph.find(id).unwrap().cache, CacheMode::None);
    assert!(!doc.graph.find(id).unwrap().disabled);

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
        let step = commit_intent(GraphIntent::SetNodeProperty { node_id: id, to }, &mut doc)
            .unwrap()
            .unwrap_or_else(|| panic!("{to:?} is a real change, not a no-op"));
        let node = doc.graph.find(id).unwrap();
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
        revert_step(&step, &mut doc);
        let node = doc.graph.find(id).unwrap();
        assert_eq!(node.cache, CacheMode::None, "revert restores the cache");
        assert!(!node.disabled, "revert restores the disable flag");
    }

    // Setting a property to the value it already holds is a no-op (no undo entry).
    for to in [
        NodeProperty::RuntimeCache(CacheMode::None),
        NodeProperty::Disabled(false),
    ] {
        assert!(
            commit_intent(GraphIntent::SetNodeProperty { node_id: id, to }, &mut doc,)
                .unwrap()
                .is_none(),
            "{to:?} equals the current value → writes nothing"
        );
    }
}

#[test]
fn commit_intent_rejects_cycle_forming_bind() {
    // a → b (b's input 0 bound to a's output 0).
    let mut fixture = DocFixture::default();
    let a = fixture.stub_at(Vec2::ZERO);
    let b = fixture.stub_at(Vec2::new(100.0, 0.0));
    let c = fixture.stub_at(Vec2::new(0.0, 100.0));
    let mut doc = fixture.doc;
    doc.graph
        .set_input_binding(InputPort::new(b, 0), Binding::bind(a, 0));

    // Wiring a's input back to b's output would close the a → b loop:
    // rejected, nothing written, the existing edge untouched.
    assert!(
        commit_intent(
            GraphIntent::SetInput {
                input: InputPort::new(a, 0),
                to: Some(Binding::bind(b, 0)),
            },
            &mut doc,
        )
        .unwrap()
        .is_none(),
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
            GraphIntent::SetInput {
                input: InputPort::new(c, 0),
                to: Some(Binding::bind(b, 0)),
            },
            &mut doc,
        )
        .unwrap()
        .is_some(),
        "an acyclic bind commits"
    );
    assert_eq!(
        doc.graph.bindings.get(&InputPort::new(c, 0)),
        Some(&Binding::bind(b, 0)),
    );
}

/// Commit `intent` expecting it to be rejected as malformed, and check nothing
/// leaked through it — the gate exists to keep a malformed payload out of
/// `apply`.
#[track_caller]
fn assert_invalid(doc: &mut Document, intent: GraphIntent, what: &str) {
    let nodes = doc.graph.len();
    match commit_intent(intent, doc) {
        Err(_) => {}
        other => panic!("{what}: expected a MalformedIntent, got {other:?}"),
    }
    assert_eq!(
        doc.graph.len(),
        nodes,
        "{what}: a refused intent must not mutate the graph"
    );
    doc.validate()
        .unwrap_or_else(|e| panic!("{what}: document invalid after a refusal: {e}"));
}

/// Commit `intent` expecting it to yield no step — the silent drop widgets
/// rely on, which is an `Ok` outcome and not an error.
#[track_caller]
fn assert_quiet(doc: &mut Document, intent: GraphIntent, what: &str) {
    match commit_intent(intent, doc) {
        Ok(None) => {}
        other => panic!("{what}: expected no step, got {other:?}"),
    }
}

fn func_node() -> Node {
    Node::new(NodeKind::Func(FuncId::unique()))
}

fn add_node(pos: Vec2, node_id: NodeId, node: Node) -> GraphIntent {
    GraphIntent::AddNode {
        pos,
        node_id,
        node,
        bindings: vec![],
    }
}

#[test]
fn insertions_reusing_an_identity_are_refused_instead_of_panicking() {
    // Both of these would otherwise abort the process inside `apply()`:
    // `AddNode` trips the absence assert in `apply_graph`, `DuplicateNodes`
    // trips `Graph::insert`'s own duplicate-id panic. Refusing at the gate
    // is what turns a would-be process abort into a stated precondition.
    let mut fixture = DocFixture::default();
    let live = fixture.stub_at(Vec2::ZERO);
    let mut doc = fixture.doc;
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
            GraphIntent::DuplicateNodes {
                nodes: vec![(Vec2::ZERO, live, func_node())],
                bindings: vec![],
                subscriptions: vec![],
            },
        ),
        (
            "DuplicateNodes repeating one id within the batch",
            GraphIntent::DuplicateNodes {
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
        assert_invalid(&mut doc, intent, what);
    }
}

#[test]
fn malformed_payloads_are_refused_before_they_can_invalidate_the_document() {
    // Every case here would apply cleanly and leave a document that
    // `Document::validate` rejects — which, because saving only validates
    // in debug builds, means a project that writes fine and won't reopen.
    let mut fixture = DocFixture::default();
    let live = fixture.stub_at(Vec2::ZERO);
    let mut doc = fixture.doc;
    let ghost = NodeId::unique();
    let nan = Vec2::new(f32::NAN, 0.0);

    let mut nil_func = func_node();
    nil_func.kind = NodeKind::Func(FuncId::nil());

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
            "AddNode seeding a binding from a producer that isn't there",
            GraphIntent::AddNode {
                pos: Vec2::ZERO,
                node_id: NodeId::unique(),
                node: func_node(),
                bindings: vec![(InputPort::new(ghost, 0), Binding::bind(live, 0))],
            },
        ),
        (
            "DuplicateNodes subscribing a node that isn't there",
            GraphIntent::DuplicateNodes {
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
            GraphIntent::MoveSelection {
                grabbed: live,
                moves: vec![(live, nan)],
            },
        ),
        (
            "SetSubscription carrying a nil id",
            GraphIntent::SetSubscription {
                emitter: NodeId::nil(),
                event_idx: 0,
                subscriber: live,
                subscribe: true,
            },
        ),
    ];
    for (what, intent) in cases {
        assert_invalid(&mut doc, intent, what);
    }
}

#[test]
fn stale_references_still_refuse_quietly() {
    // Widgets emit identities they read out of the live document, so the
    // only thing they get wrong is staleness — an anchor removed between
    // the gesture starting and the intent draining. Those stay silent;
    // turning them into reported failures would spam the status bar on
    // ordinary use.
    let mut fixture = DocFixture::default();
    let live = fixture.stub_at(Vec2::ZERO);
    let mut doc = fixture.doc;
    let gone = NodeId::unique();

    let cases = [
        ("RemoveNode", GraphIntent::RemoveNode { node_id: gone }),
        (
            "RenameNode",
            GraphIntent::RenameNode {
                node_id: gone,
                to: "x".into(),
            },
        ),
        (
            "SetNodeProperty",
            GraphIntent::SetNodeProperty {
                node_id: gone,
                to: NodeProperty::Disabled(true),
            },
        ),
        ("Raise", GraphIntent::Raise { key: gone }),
        (
            "SetInput onto a vanished node",
            GraphIntent::SetInput {
                input: InputPort::new(gone, 0),
                to: None,
            },
        ),
        (
            // The held-wire case: the producer was removed after the drag
            // began, so committing would leave a dangling edge.
            "SetInput from a vanished producer",
            GraphIntent::SetInput {
                input: InputPort::new(live, 0),
                to: Some(Binding::bind(gone, 0)),
            },
        ),
        (
            // An event wire dropped on a node that's since gone: dropped
            // rather than recorded as a dangling subscription.
            "SetSubscription onto a vanished subscriber",
            GraphIntent::SetSubscription {
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
            GraphIntent::MoveSelection {
                grabbed: gone,
                moves: vec![(gone, Vec2::ZERO)],
            },
        ),
    ];
    for (what, intent) in cases {
        assert_quiet(&mut doc, intent, what);
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
    let mut fixture = DocFixture::default();
    let live = fixture.stub_at(Vec2::ZERO);
    let mut doc = fixture.doc;
    let gone = NodeId::unique();

    let step = commit_intent(
        GraphIntent::SetSelection {
            to: [live, gone].into_iter().collect(),
        },
        &mut doc,
    )
    .unwrap()
    .expect("a selection with one live member commits");
    let UndoStep::SetSelection { to, .. } = &step else {
        panic!("expected a SetSelection step, got {step:?}");
    };
    assert_eq!(
        to,
        &[live].into_iter().collect::<BTreeSet<_>>(),
        "the vanished member is dropped, the live one kept"
    );
    assert_eq!(doc.main_view.selected, *to);

    let step = commit_intent(
        GraphIntent::MoveSelection {
            grabbed: live,
            moves: vec![(live, Vec2::new(5.0, 6.0)), (gone, Vec2::new(7.0, 8.0))],
        },
        &mut doc,
    )
    .unwrap()
    .expect("a move with one live member commits");
    let UndoStep::MoveSelection { moves, .. } = &step else {
        panic!("expected a MoveSelection step, got {step:?}");
    };
    assert_eq!(
        moves,
        &[(live, Vec2::ZERO, Vec2::new(5.0, 6.0))],
        "only the surviving member is recorded"
    );
    doc.validate().expect("document stays valid");
}
