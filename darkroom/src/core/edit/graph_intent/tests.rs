use std::collections::BTreeSet;

use glam::Vec2;
use scenarium::{
    Binding, CacheMode, ConstValue, FuncId, InputPort, Node, NodeId, NodeKind, Subscription,
};

use super::{DUPLICATE_OFFSET, GraphIntent};
use crate::core::document::harness::DocFixture;
use crate::core::document::{Document, Viewport};
use crate::core::edit::step::set_node_property::NodeProperty;
use crate::core::edit::step::undo_step::UndoStep;

/// Commit a whole list of intents the way one frame's drain does — each built
/// against the document the one before it left — and hand back the steps in
/// the order they applied, so a test can undo the lot by walking them back.
#[track_caller]
fn commit_all(doc: &mut Document, intents: impl IntoIterator<Item = GraphIntent>) -> Vec<UndoStep> {
    intents
        .into_iter()
        .filter_map(|intent| intent.commit(doc).expect("the batch is well formed"))
        .collect()
}

#[track_caller]
fn undo_all(doc: &mut Document, steps: &[UndoStep]) {
    for step in steps.iter().rev() {
        step.revert(doc);
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

fn subscribe(emitter: NodeId, subscriber: NodeId, subscribe: bool) -> GraphIntent {
    GraphIntent::SetSubscription {
        subscription: Subscription {
            emitter,
            event_idx: 0,
            subscriber,
        },
        subscribe,
    }
}

/// A plain click always lifts its node to the front; a Shift-click that
/// *removes* a node must not, or deselecting would jump it forward.
#[test]
fn click_raises_unless_shift_deselects() {
    let (a, b) = (NodeId::unique(), NodeId::unique());
    let sel = |ids: &[NodeId]| ids.iter().copied().collect::<BTreeSet<_>>();
    let click = |shift, selected: BTreeSet<NodeId>, key| {
        GraphIntent::click(shift, &selected, key).collect::<Vec<_>>()
    };

    // Plain click on an unselected node: select it, then raise it.
    let out = click(false, sel(&[]), a);
    assert_eq!(out.len(), 2);
    assert!(matches!(out[0], GraphIntent::SetSelection { .. }));
    assert!(matches!(out[1], GraphIntent::Raise { key } if key == a));

    // Plain click on an already-selected node still raises it.
    let out = click(false, sel(&[a]), a);
    assert!(
        out.iter()
            .any(|i| matches!(i, GraphIntent::Raise { key } if *key == a)),
        "a plain click always lifts its node to the front"
    );

    // Shift-click adding a fresh node to the selection raises it.
    let out = click(true, sel(&[a]), b);
    assert!(
        out.iter()
            .any(|i| matches!(i, GraphIntent::Raise { key } if *key == b)),
        "shift-adding a node raises it"
    );

    // Shift-click removing a node does NOT raise it.
    let out = click(true, sel(&[a, b]), b);
    assert_eq!(out.len(), 1, "shift-deselect suppresses the raise");
    assert!(matches!(out[0], GraphIntent::SetSelection { .. }));
}

/// Removing a node and undoing it is the one edit that has to put back state
/// from three places at once — the graph record with every edge that touched
/// the node, its placement, and its selection membership. All three come back
/// exactly, which is what lets the step assert its own removal against them.
#[test]
fn a_removed_node_comes_back_whole() {
    let mut fixture = DocFixture::default();
    let a = fixture.stub_at(Vec2::new(10.0, 20.0));
    let b = fixture.stub_at(Vec2::new(100.0, 0.0));
    let mut doc = fixture.doc;
    let edge = InputPort::new(b, 0);
    doc.graph.set_input_binding(edge, Binding::bind(a, 0));
    doc.graph.subscribe(a, 0, b);
    doc.main_view.selected = [a].into_iter().collect();
    let placement = doc.main_view.item_placements[&a];

    let step = GraphIntent::RemoveNode { node_id: a }
        .commit(&mut doc)
        .unwrap()
        .expect("removing a live node commits");
    assert!(doc.graph.find(a).is_none());
    assert!(!doc.main_view.item_placements.contains_key(&a));
    assert!(!doc.main_view.selected.contains(&a), "selection is pruned");
    assert_eq!(doc.graph.bindings.get(&edge), None, "the edge went with it");
    assert!(!doc.graph.is_subscribed(a, 0, b), "so did the subscription");
    doc.validate().expect("a removal leaves a valid document");

    step.revert(&mut doc);
    assert!(doc.graph.find(a).is_some());
    assert_eq!(
        doc.main_view.item_placements[&a], placement,
        "position *and* paint depth come back, not a fresh frontmost slot"
    );
    assert!(doc.main_view.selected.contains(&a));
    assert_eq!(doc.graph.bindings.get(&edge), Some(&Binding::bind(a, 0)));
    assert!(doc.graph.is_subscribed(a, 0, b));
    doc.validate()
        .expect("an undone removal leaves a valid document");

    // Redo takes it out again — and the step checks what came out against
    // what it restored, so a divergence would fail here rather than silently
    // losing an edge on the next undo.
    step.apply(&mut doc);
    assert!(doc.graph.find(a).is_none());
    assert_eq!(doc.graph.bindings.get(&edge), None);
}

/// A new node's depth is fixed when its step is built, not when the step is
/// written — so replaying history puts it back where it was rather than in
/// front of whatever has arrived since.
///
/// The divergence is only visible once a later entry restacks something:
/// add `x` at the front, raise `y` past it, then walk both back and forward
/// again. Reading `front_z()` at write time would redo the add at `y`'s depth
/// and swap the two in the paint order.
#[test]
fn redo_restores_the_depth_an_add_had() {
    let mut fixture = DocFixture::default();
    let y = fixture.stub_at(Vec2::ZERO);
    let mut doc = fixture.doc;
    let x = NodeId::unique();

    let add = GraphIntent::AddNode {
        pos: Vec2::new(50.0, 0.0),
        node_id: x,
        node: func_node(),
        bindings: vec![],
    }
    .commit(&mut doc)
    .unwrap()
    .expect("adding a fresh node commits");
    let x_z = doc.main_view.item_placements[&x].z;
    let raise = GraphIntent::Raise { key: y }
        .commit(&mut doc)
        .unwrap()
        .expect("y is behind x, so raising it is a real change");
    assert!(
        doc.main_view.item_placements[&y].z > x_z,
        "the raise put y in front"
    );

    // Undo both, then redo both — the order the action stack replays in.
    raise.revert(&mut doc);
    add.revert(&mut doc);
    add.apply(&mut doc);
    assert_eq!(
        doc.main_view.item_placements[&x].z, x_z,
        "the redone add keeps its recorded depth"
    );
    raise.apply(&mut doc);
    assert!(
        doc.main_view.item_placements[&y].z > doc.main_view.item_placements[&x].z,
        "so the paint order after a round trip is the one it started with"
    );
}

/// `a -> b` inside the selection, `c -> b` crossing out of it: the fixture the
/// two duplicate tests share, so the only thing that differs between them is
/// the `include_incoming` flag. `b` also carries a Const on input 1, and `a`
/// emits to `b`.
fn crossing_wire() -> (Document, NodeId, NodeId, NodeId) {
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
    doc.graph.subscribe(a, 0, b);
    doc.main_view.selected = [a, b].into_iter().collect();
    (doc, a, b, c)
}

/// The clone of `b` in a document whose only unselected node is `c`.
#[track_caller]
fn clone_of(doc: &Document, origin: Vec2, originals: &BTreeSet<NodeId>) -> NodeId {
    doc.graph
        .iter()
        .map(|node| node.id)
        .find(|id| {
            !originals.contains(id)
                && doc.main_view.item_placements[id].pos == origin + DUPLICATE_OFFSET
        })
        .unwrap_or_else(|| panic!("a clone offset from {origin:?}"))
}

/// Duplicating expands into ordinary intents — one add per clone, one
/// `SetInput` per edge *inside* the set, the internal subscriptions, then the
/// selection swap — so the whole thing commits through the same gate as any
/// other edit and undoes as one batch.
#[test]
fn duplicate_clones_wiring_and_selects_the_copies() {
    let (mut doc, a, b, c) = crossing_wire();
    let originals: BTreeSet<NodeId> = [a, b, c].into_iter().collect();

    let intents = GraphIntent::duplicate(&doc, false);
    let steps = commit_all(&mut doc, intents);
    doc.validate().expect("a duplicate leaves a valid document");

    // Two fresh nodes, offset from their originals, and they are what is
    // selected now.
    let clones: BTreeSet<NodeId> = doc
        .graph
        .iter()
        .map(|node| node.id)
        .filter(|id| !originals.contains(id))
        .collect();
    assert_eq!(clones.len(), 2, "both selected nodes were cloned");
    assert_eq!(doc.main_view.selected, clones, "the copies end up selected");
    let a_clone = clone_of(&doc, Vec2::ZERO, &originals);
    let b_clone = clone_of(&doc, Vec2::new(100.0, 0.0), &originals);

    // The internal edge is recreated against the clones, the const copies
    // verbatim, the edge from outside the set is dropped, and the internal
    // subscription is recreated.
    assert_eq!(
        doc.graph.bindings.get(&InputPort::new(b_clone, 0)),
        Some(&Binding::bind(a_clone, 0)),
        "a' -> b' replaces a -> b"
    );
    assert!(matches!(
        doc.graph.bindings.get(&InputPort::new(b_clone, 1)),
        Some(Binding::Const(_))
    ));
    assert_eq!(
        doc.graph.bindings.get(&InputPort::new(b_clone, 2)),
        None,
        "the edge from unselected c is dropped"
    );
    assert!(doc.graph.is_subscribed(a_clone, 0, b_clone));

    // One batch, so one undo: every clone and its wiring goes, and the
    // selection is the one the duplicate replaced.
    undo_all(&mut doc, &steps);
    assert_eq!(doc.graph.len(), 3, "only the originals are left");
    assert_eq!(doc.main_view.selected, [a, b].into_iter().collect());
    assert_eq!(
        doc.graph.bindings.get(&InputPort::new(b, 2)),
        Some(&Binding::bind(c, 0)),
        "the originals' wiring was never touched"
    );
    doc.validate()
        .expect("an undone duplicate leaves a valid document");

    // ...and redo replays it forward: each clone is attached again, then the
    // edges between them, so nothing is bound to a node that isn't back yet.
    for step in &steps {
        step.apply(&mut doc);
    }
    assert_eq!(doc.graph.len(), 5);
    assert_eq!(doc.main_view.selected, clones);
    assert_eq!(
        doc.graph.bindings.get(&InputPort::new(b_clone, 0)),
        Some(&Binding::bind(a_clone, 0))
    );
    doc.validate()
        .expect("a redone duplicate leaves a valid document");
}

/// The same selection over the same crossing wire, with `include_incoming`:
/// the clone keeps the wire from the producer outside the set — pointing at
/// the original, since that producer wasn't copied. The flag is the only
/// difference from the case above, where the same edge is dropped.
#[test]
fn duplicate_keeps_external_producers_on_request() {
    let (mut doc, a, b, c) = crossing_wire();
    let originals: BTreeSet<NodeId> = [a, b, c].into_iter().collect();

    let intents = GraphIntent::duplicate(&doc, true);
    commit_all(&mut doc, intents);
    let b_clone = clone_of(&doc, Vec2::new(100.0, 0.0), &originals);
    assert_eq!(
        doc.graph.bindings.get(&InputPort::new(b_clone, 2)),
        Some(&Binding::bind(c, 0)),
        "the kept wire still reads the original external producer"
    );
    doc.validate().unwrap();
}

#[test]
fn duplicate_of_nothing_asks_for_nothing() {
    let mut fixture = DocFixture::default();
    fixture.stub_at(Vec2::ZERO);
    assert!(GraphIntent::duplicate(&fixture.doc, false).is_empty());
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
            GraphIntent::SetViewport { to }.commit(&mut doc).is_err(),
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
        GraphIntent::SetViewport { to: valid }
            .commit(&mut doc)
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

    // Subscribe commits and writes the edge.
    let step = subscribe(emitter, subscriber, true)
        .commit(&mut doc)
        .unwrap()
        .expect("subscribe commits");
    assert!(doc.graph.is_subscribed(emitter, 0, subscriber));

    // A second identical subscribe is a no-op (from == to == true).
    assert!(
        subscribe(emitter, subscriber, true)
            .commit(&mut doc)
            .unwrap()
            .is_none(),
        "re-subscribing the same edge is a no-op"
    );

    // Undo removes it; redo restores it.
    step.revert(&mut doc);
    assert!(!doc.graph.is_subscribed(emitter, 0, subscriber));
    step.apply(&mut doc);
    assert!(doc.graph.is_subscribed(emitter, 0, subscriber));

    // Unsubscribe commits, removes the edge, and undo brings it back.
    let step = subscribe(emitter, subscriber, false)
        .commit(&mut doc)
        .unwrap()
        .expect("unsubscribe commits");
    assert!(!doc.graph.is_subscribed(emitter, 0, subscriber));
    step.revert(&mut doc);
    assert!(doc.graph.is_subscribed(emitter, 0, subscriber));

    // Redo the unsubscribe (apply writes the `to = unsubscribed` half),
    // then unsubscribing the now-absent edge is a no-op.
    step.apply(&mut doc);
    assert!(!doc.graph.is_subscribed(emitter, 0, subscriber));
    assert!(
        subscribe(emitter, subscriber, false)
            .commit(&mut doc)
            .unwrap()
            .is_none(),
        "unsubscribing a missing edge is a no-op"
    );
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
        let step = GraphIntent::SetNodeProperty { node_id: id, to }
            .commit(&mut doc)
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
        step.revert(&mut doc);
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
            GraphIntent::SetNodeProperty { node_id: id, to }
                .commit(&mut doc)
                .unwrap()
                .is_none(),
            "{to:?} equals the current value → writes nothing"
        );
    }
}

#[test]
fn commit_rejects_cycle_forming_bind() {
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
        GraphIntent::SetInput {
            input: InputPort::new(a, 0),
            to: Some(Binding::bind(b, 0)),
        }
        .commit(&mut doc)
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
        GraphIntent::SetInput {
            input: InputPort::new(c, 0),
            to: Some(Binding::bind(b, 0)),
        }
        .commit(&mut doc)
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
/// leaked through it — the gate exists to keep a malformed payload out of the
/// write half.
#[track_caller]
fn assert_invalid(doc: &mut Document, intent: GraphIntent, what: &str) {
    let nodes = doc.graph.len();
    match intent.commit(doc) {
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
    match intent.commit(doc) {
        Ok(None) => {}
        other => panic!("{what}: expected no step, got {other:?}"),
    }
}

#[test]
fn insertions_reusing_an_identity_are_refused_instead_of_panicking() {
    // Both of these would otherwise abort the process inside the write half:
    // a live id trips `Graph::insert`'s own duplicate-id panic, and a nil one
    // trips the assert in `Graph::find`. Refusing at the gate is what turns a
    // would-be process abort into a stated precondition.
    let mut fixture = DocFixture::default();
    let live = fixture.stub_at(Vec2::ZERO);
    let mut doc = fixture.doc;

    assert_invalid(
        &mut doc,
        add_node(Vec2::ZERO, live, func_node()),
        "AddNode over a live id",
    );
    assert_invalid(
        &mut doc,
        add_node(Vec2::ZERO, NodeId::nil(), func_node()),
        "AddNode with a nil id",
    );

    // Intents commit one at a time, so an id repeated *within* one batch is
    // refused by the same check: the second add meets a graph the first is
    // already in.
    let repeated = NodeId::unique();
    assert!(
        add_node(Vec2::ZERO, repeated, func_node())
            .commit(&mut doc)
            .unwrap()
            .is_some()
    );
    assert_invalid(
        &mut doc,
        add_node(Vec2::ONE, repeated, func_node()),
        "a second AddNode repeating an id from the same batch",
    );
}

#[test]
fn malformed_payloads_are_refused_before_they_can_invalidate_the_document() {
    // Every case here would apply cleanly and leave a document that
    // `Document::validate` rejects, or trip an assert inside the graph — which,
    // because saving only validates in debug builds, means a project that
    // writes fine and won't reopen.
    let mut fixture = DocFixture::default();
    let live = fixture.stub_at(Vec2::ZERO);
    let mut doc = fixture.doc;
    let ghost = NodeId::unique();
    let nan = Vec2::new(f32::NAN, 0.0);

    let mut nil_func = func_node();
    nil_func.kind = NodeKind::Func(FuncId::nil());

    let fresh = NodeId::unique();
    let seeded = |bindings: Vec<(InputPort, Binding)>| GraphIntent::AddNode {
        pos: Vec2::ZERO,
        node_id: fresh,
        node: func_node(),
        bindings,
    };
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
            seeded(vec![(InputPort::new(fresh, 0), Binding::bind(ghost, 0))]),
        ),
        (
            // The insertion restores exactly the wiring it recorded, so it may
            // only author its own node's inputs.
            "AddNode seeding a binding on someone else's port",
            seeded(vec![(InputPort::new(live, 0), Binding::bind(fresh, 0))]),
        ),
        (
            // A loop of one: nothing reads the new node yet, so its own output
            // is the only source that could close a cycle — and the planner
            // refuses a cyclic graph outright.
            "AddNode seeding an input from the new node's own output",
            seeded(vec![(InputPort::new(fresh, 0), Binding::bind(fresh, 0))]),
        ),
        (
            // Only one could survive, and the record would then disagree with
            // the graph — `Graph::attach_node` asserts on the malformed record.
            "AddNode seeding one port twice",
            seeded(vec![
                (
                    InputPort::new(fresh, 0),
                    Binding::Const(ConstValue::from(1i64)),
                ),
                (
                    InputPort::new(fresh, 0),
                    Binding::Const(ConstValue::from(2i64)),
                ),
            ]),
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
            subscribe(NodeId::nil(), live, true),
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
            subscribe(live, gone, true),
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

    let step = GraphIntent::SetSelection {
        to: [live, gone].into_iter().collect(),
    }
    .commit(&mut doc)
    .unwrap()
    .expect("a selection with one live member commits");
    let UndoStep::SetSelection(step) = &step else {
        panic!("expected a SetSelection step, got {step:?}");
    };
    assert_eq!(
        step.selection.to,
        [live].into_iter().collect::<BTreeSet<_>>(),
        "the vanished member is dropped, the live one kept"
    );
    assert_eq!(doc.main_view.selected, step.selection.to);

    let step = GraphIntent::MoveSelection {
        grabbed: live,
        moves: vec![(live, Vec2::new(5.0, 6.0)), (gone, Vec2::new(7.0, 8.0))],
    }
    .commit(&mut doc)
    .unwrap()
    .expect("a move with one live member commits");
    let UndoStep::MoveSelection(step) = &step else {
        panic!("expected a MoveSelection step, got {step:?}");
    };
    assert_eq!(step.moves.len(), 1, "only the surviving member is recorded");
    assert_eq!(step.moves[0].key, live);
    assert_eq!(
        (step.moves[0].pos.from, step.moves[0].pos.to),
        (Vec2::ZERO, Vec2::new(5.0, 6.0))
    );
    doc.validate().expect("document stays valid");
}
