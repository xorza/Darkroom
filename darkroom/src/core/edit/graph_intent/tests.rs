use std::collections::BTreeSet;

use glam::Vec2;
use scenarium::internals::test_graph::TestGraph;
use scenarium::{
    Binding, BindingEntry, BindingRejection, CacheMode, ConstValue, DataType, EventPort, FuncId,
    FuncInput, FuncOutput, InputPort, Library, Node, NodeId, OutputPort, Subscription,
    SubscriptionRejection, internals,
};

use crate::core::document::Document;
use crate::core::document::internals::DocumentFixture;
use crate::core::document::item_id::ItemId;
use crate::core::document::route::Route;
use crate::core::document::tie::{Tie, TieAngle, TieId};
use crate::core::document::viewport::Viewport;
use crate::core::document::wire_key::{DataWire, WireKey};
use crate::core::edit::error::{MalformedIntent, Role};
use crate::core::edit::gesture_id::GestureId;
use crate::core::edit::graph_intent::click_mode::ClickMode;
use crate::core::edit::graph_intent::drag_start::DragStart;
use crate::core::edit::graph_intent::incoming_wires::IncomingWires;
use crate::core::edit::graph_intent::route_insert::RouteInsert;
use crate::core::edit::graph_intent::{DUPLICATE_OFFSET, GraphIntent};
use crate::core::edit::step::set_node_property::NodeProperty;
use crate::core::edit::step::undo_step::UndoStep;

fn member(node: NodeId, pos: Vec2) -> DragStart {
    DragStart {
        item: ItemId::Node(node),
        pos,
    }
}

/// One frame of a drag: every member moves to its start plus `offset`.
fn drag(members: &[DragStart], offset: Vec2) -> GraphIntent {
    GraphIntent::MoveSelection {
        gesture: GestureId::default().next(),
        members: members.into(),
        offset,
    }
}

/// Commit a whole list of intents the way one frame's drain does — each built
/// against the document the one before it left — and hand back the steps in
/// the order they applied, so a test can undo the lot by walking them back.
#[track_caller]
fn commit_all(
    document: &mut Document,
    library: &Library,
    intents: impl IntoIterator<Item = GraphIntent>,
) -> Vec<UndoStep> {
    let mut steps = Vec::new();
    for intent in intents {
        intent
            .commit(document, library, &mut steps)
            .expect("the batch is well formed");
    }
    steps
}

#[track_caller]
fn undo_all(document: &mut Document, steps: &[UndoStep]) {
    for step in steps.iter().rev() {
        step.revert(document);
    }
}

fn subscribe(emitter: NodeId, subscriber: NodeId, subscribe: bool) -> GraphIntent {
    GraphIntent::SetSubscription {
        subscription: Subscription::new(EventPort::new(emitter, 0), subscriber),
        subscribe,
    }
}

/// A plain click always lifts its node to the front; a Shift-click that
/// *removes* a node must not, or deselecting would jump it forward.
#[test]
fn click_raises_unless_shift_deselects() {
    let (a, b) = (NodeId::unique(), NodeId::unique());
    let sel = |ids: &[NodeId]| {
        ids.iter()
            .copied()
            .map(ItemId::Node)
            .collect::<BTreeSet<_>>()
    };
    let click = |mode, selected: BTreeSet<ItemId>, node_id| {
        GraphIntent::click(mode, &selected, ItemId::Node(node_id)).collect::<Vec<_>>()
    };

    // Plain click on an unselected node: select it, then raise it.
    let out = click(ClickMode::Replace, sel(&[]), a);
    assert_eq!(out.len(), 2);
    assert!(matches!(out[0], GraphIntent::SetSelection { .. }));
    assert!(matches!(out[1], GraphIntent::Raise { node_id } if node_id == a));

    // Plain click on an already-selected node still raises it.
    let out = click(ClickMode::Replace, sel(&[a]), a);
    assert!(
        out.iter()
            .any(|i| matches!(i, GraphIntent::Raise { node_id } if *node_id == a)),
        "a plain click always lifts its node to the front"
    );

    // Shift-click adding a fresh node to the selection raises it.
    let out = click(ClickMode::Toggle, sel(&[a]), b);
    assert!(
        out.iter()
            .any(|i| matches!(i, GraphIntent::Raise { node_id } if *node_id == b)),
        "shift-adding a node raises it"
    );

    // Shift-click removing a node does NOT raise it.
    let out = click(ClickMode::Toggle, sel(&[a, b]), b);
    assert_eq!(out.len(), 1, "shift-deselect suppresses the raise");
    assert!(matches!(out[0], GraphIntent::SetSelection { .. }));
}

/// Removing a node and undoing it is the one edit that has to put back state
/// from three places at once — the graph record with every edge that touched
/// the node, its placement, and its selection membership. All three come back
/// exactly, which is what lets the step assert its own removal against them.
/// A stub goes and comes back the same way, to the stubs rather than the
/// graph.
#[test]
fn a_removed_node_comes_back_whole() {
    let mut fixture = DocumentFixture::default();
    let a = fixture.node_at(Vec2::new(10.0, 20.0));
    let b = fixture.node_at(Vec2::new(100.0, 0.0));
    let stub = fixture.stub_at(Vec2::new(0.0, 100.0));
    let mut document = fixture.document;
    let edge = InputPort::new(b, 0);
    document.graph.set_input_binding(edge, Binding::wire(a, 0));
    document.graph.subscribe(EventPort::new(a, 0), b);
    document.main_view.selected = [a].map(ItemId::Node).into_iter().collect();
    let placement = document.main_view.node_placements[&a];

    let step = GraphIntent::RemoveNode { node_id: a }
        .commit_one(&mut document, &fixture.library)
        .unwrap()
        .expect("removing a live node commits");
    assert!(document.graph.find(a).is_none());
    assert!(!document.main_view.node_placements.contains_key(&a));
    assert!(
        !document.main_view.selected.contains(&ItemId::Node(a)),
        "selection is pruned"
    );
    assert_eq!(
        document.graph.bindings.get(&edge),
        None,
        "the edge went with it"
    );
    assert!(
        !document.graph.is_subscribed(EventPort::new(a, 0), b),
        "so did the subscription"
    );
    document
        .validate()
        .expect("a removal leaves a valid document");

    step.revert(&mut document);
    assert!(document.graph.find(a).is_some());
    assert_eq!(
        document.main_view.node_placements[&a], placement,
        "position *and* paint depth come back, not a fresh frontmost slot"
    );
    assert!(document.main_view.selected.contains(&ItemId::Node(a)));
    assert_eq!(
        document.graph.bindings.get(&edge),
        Some(&Binding::wire(a, 0))
    );
    assert!(document.graph.is_subscribed(EventPort::new(a, 0), b));
    document
        .validate()
        .expect("an undone removal leaves a valid document");

    // Redo takes it out again — and the step checks what came out against
    // what it restored, so a divergence would fail here rather than silently
    // losing an edge on the next undo.
    step.apply(&mut document);
    assert!(document.graph.find(a).is_none());
    assert_eq!(document.graph.bindings.get(&edge), None);

    let record = document.stubs.get(stub).cloned().unwrap();
    let stub_placement = document.main_view.node_placements[&stub];
    let step = GraphIntent::RemoveNode { node_id: stub }
        .commit_one(&mut document, &fixture.library)
        .unwrap()
        .expect("removing a stub commits");
    assert!(!document.stubs.contains(stub));
    assert!(!document.main_view.node_placements.contains_key(&stub));
    document
        .validate()
        .expect("a stub's removal leaves a valid document");
    step.revert(&mut document);
    assert_eq!(document.stubs.get(stub), Some(&record));
    assert_eq!(document.main_view.node_placements[&stub], stub_placement);
    assert!(
        document.graph.find(stub).is_none(),
        "a stub stays out of the graph"
    );
    document
        .validate()
        .expect("an undone stub removal leaves a valid document");
}

/// A wire that goes leaves its input on the input's declared default, so a
/// preset input shows its pick again: cut on its own, dropped from a
/// duplicate, or gone with the node that fed it. An input with no default is
/// left unbound, and clearing a constant leaves it unbound too. Undo puts
/// each wire back, and redo the default.
#[test]
fn a_wire_that_goes_leaves_its_input_on_its_default() {
    let mut fixture = DocumentFixture::default();
    let consumer = internals::stub_func(FuncId::unique(), "consumer")
        .input(FuncInput::optional("knob", DataType::Int).default(7i64))
        .input(FuncInput::optional("bare", DataType::Int))
        .build();
    let producer = fixture.node_at(Vec2::ZERO);
    let node = fixture.add(&consumer);
    let library = fixture.library;
    let mut document = fixture.document;
    let (knob, bare) = (InputPort::new(node, 0), InputPort::new(node, 1));
    let wire = || Some(Binding::wire(producer, 0));
    let seven = || Some(Binding::Const(ConstValue::Int(7)));
    let wire_both = |document: &mut Document| {
        for port in [knob, bare] {
            document.graph.set_input_binding(port, wire());
        }
    };
    let bindings =
        |document: &Document| [knob, bare].map(|port| document.graph.bindings.get(&port).cloned());

    wire_both(&mut document);
    let cut = commit_all(
        &mut document,
        &library,
        [knob, bare].map(|input| GraphIntent::SetInput { input, to: None }),
    );
    assert_eq!(bindings(&document), [seven(), None]);
    undo_all(&mut document, &cut);
    assert_eq!(bindings(&document), [wire(), wire()]);
    for step in &cut {
        step.apply(&mut document);
    }
    assert_eq!(bindings(&document), [seven(), None]);
    commit_all(
        &mut document,
        &library,
        [GraphIntent::SetInput {
            input: knob,
            to: None,
        }],
    );
    assert_eq!(
        bindings(&document),
        [None, None],
        "a cleared constant is unbound"
    );

    wire_both(&mut document);
    document.main_view.selected = [node].map(ItemId::Node).into_iter().collect();
    let intents = GraphIntent::duplicate(&document, &library, IncomingWires::Drop);
    let copy = commit_all(&mut document, &library, intents);
    let clone = document
        .main_view
        .selected
        .first()
        .and_then(|item| item.node())
        .unwrap();
    assert_ne!(clone, node);
    assert_eq!(
        [0, 1].map(|port_idx| document
            .graph
            .bindings
            .get(&InputPort::new(clone, port_idx))
            .cloned()),
        [seven(), None],
        "a duplicate drops the wires from outside it as a cut does"
    );
    undo_all(&mut document, &copy);

    let removal = commit_all(
        &mut document,
        &library,
        [GraphIntent::RemoveNode { node_id: producer }],
    );
    assert!(
        matches!(removal.as_slice(), [UndoStep::Node(_), UndoStep::Input(_)]),
        "the removal, then the one input with a default taking it back: {removal:?}"
    );
    assert_eq!(bindings(&document), [seven(), None]);
    document
        .validate()
        .expect("a removal leaves a valid document");
    undo_all(&mut document, &removal);
    assert_eq!(bindings(&document), [wire(), wire()]);
    document
        .validate()
        .expect("an undone removal leaves a valid document");
    for step in &removal {
        step.apply(&mut document);
    }
    assert_eq!(bindings(&document), [seven(), None]);
}

/// The gate holds every write to [`scenarium::Graph::admit`]: a constant the
/// input does not accept, a wire whose output type it does not accept, and a
/// node listening to its own event are refused, and nothing is written.
#[test]
fn the_gate_refuses_a_mistyped_constant_or_wire_and_a_self_subscription() {
    let mut g = TestGraph::new();
    let src = g.add("src", |n| {
        n.output(DataType::String)
            .event("tick", internals::stub_event())
    });
    let dst = g.add("dst", |n| n.sink().input(DataType::Int));
    let DocumentFixture {
        mut document,
        library,
    } = DocumentFixture::from(g);
    let input = InputPort::new(dst, 0);
    let refused = |intent: GraphIntent, document: &mut Document| {
        intent
            .commit_one(document, &library)
            .expect_err("the gate refuses it")
    };

    assert!(matches!(
        refused(
            GraphIntent::set_input(input, Binding::Const(ConstValue::from("text"))),
            &mut document
        ),
        MalformedIntent::Rejected { port, rejection: BindingRejection::ConstMismatch }
            if port == input
    ));
    assert!(matches!(
        refused(GraphIntent::set_input(input, Binding::wire(src, 0)), &mut document),
        MalformedIntent::Rejected { port, rejection: BindingRejection::WireMismatch }
            if port == input
    ));
    let own = Subscription::new(EventPort::new(src, 0), src);
    assert!(matches!(
        refused(
            GraphIntent::SetSubscription {
                subscription: own,
                subscribe: true,
            },
            &mut document
        ),
        MalformedIntent::RejectedSubscription {
            subscription,
            rejection: SubscriptionRejection::SelfSubscription,
        } if subscription == own
    ));
    assert!(document.graph.bindings.is_empty());
    assert_eq!(document.graph.subscriptions().len(), 0);
}

/// Duplicate copies the selected graph nodes and skips a selected stub: a
/// copy of a node that cannot run has little use.
#[test]
fn duplicate_skips_a_selected_stub() {
    let mut fixture = DocumentFixture::default();
    let node = fixture.node_at(Vec2::ZERO);
    let stub = fixture.stub_at(Vec2::new(100.0, 0.0));
    let DocumentFixture {
        mut document,
        library,
    } = fixture;
    document.main_view.selected = [node, stub].map(ItemId::Node).into_iter().collect();

    let intents = GraphIntent::duplicate(&document, &library, IncomingWires::Drop);
    commit_all(&mut document, &library, intents);
    assert_eq!(document.graph.len(), 2, "the graph node was copied");
    assert_eq!(document.stubs.len(), 1, "the stub was not");
    document
        .validate()
        .expect("a duplicate leaves a valid document");
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
    let mut fixture = DocumentFixture::default();
    let y = fixture.node_at(Vec2::ZERO);
    let node = fixture.plain_node();
    let mut document = fixture.document;
    let x = NodeId::unique();

    let add = GraphIntent::add_node(Vec2::new(50.0, 0.0), x, node)
        .commit_one(&mut document, &fixture.library)
        .unwrap()
        .expect("adding a fresh node commits");
    let x_z = document.main_view.node_placements[&x].z;
    let raise = GraphIntent::Raise { node_id: y }
        .commit_one(&mut document, &fixture.library)
        .unwrap()
        .expect("y is behind x, so raising it is a real change");
    assert!(
        document.main_view.node_placements[&y].z > x_z,
        "the raise put y in front"
    );

    // Undo both, then redo both — the order the undo history replays in.
    raise.revert(&mut document);
    add.revert(&mut document);
    add.apply(&mut document);
    assert_eq!(
        document.main_view.node_placements[&x].z, x_z,
        "the redone add keeps its recorded depth"
    );
    raise.apply(&mut document);
    assert!(
        document.main_view.node_placements[&y].z > document.main_view.node_placements[&x].z,
        "so the paint order after a round trip is the one it started with"
    );
}

/// Raising what already paints last changes nothing, so it records nothing: a click on the
/// frontmost node must not cost the user an undo entry that undoes nothing. The tie case matters
/// too: on equal depth the higher id paints last, so raising it is a no-op and raising the lower id
/// is not.
#[test]
fn raising_the_frontmost_item_records_no_step() {
    let mut fixture = DocumentFixture::default();
    let first = fixture.node_at(Vec2::ZERO);
    let second = fixture.node_at(Vec2::new(50.0, 0.0));
    let mut document = fixture.document;
    for node_id in [first, second] {
        document
            .main_view
            .node_placements
            .get_mut(&node_id)
            .unwrap()
            .z = 4;
    }
    let (low, high) = if first < second {
        (first, second)
    } else {
        (second, first)
    };

    for _ in 0..3 {
        assert!(
            GraphIntent::Raise { node_id: high }
                .commit_one(&mut document, &fixture.library)
                .unwrap()
                .is_none(),
            "the higher id already paints last at a tied depth"
        );
    }
    assert_eq!(document.main_view.node_placements[&high].z, 4);

    let step = GraphIntent::Raise { node_id: low }
        .commit_one(&mut document, &fixture.library)
        .unwrap()
        .expect("the lower id is behind at a tied depth");
    assert_eq!(document.main_view.node_placements[&low].z, 5);
    assert!(
        GraphIntent::Raise { node_id: low }
            .commit_one(&mut document, &fixture.library)
            .unwrap()
            .is_none(),
        "once in front, raising again is a no-op"
    );
    step.revert(&mut document);
    assert_eq!(document.main_view.node_placements[&low].z, 4);
}

/// `a -> b` inside the selection, `c -> b` crossing out of it: the fixture the
/// two duplicate tests share, so the only thing that differs between them is
/// their `IncomingWires`. `b` also carries a Const on input 1, and `a`
/// emits to `b`.
#[derive(Debug)]
struct CrossingWire {
    document: Document,
    a: NodeId,
    b: NodeId,
    c: NodeId,
    library: Library,
}

fn crossing_wire() -> CrossingWire {
    let mut fixture = DocumentFixture::default();
    let a = fixture.node_at(Vec2::new(0.0, 0.0));
    let b = fixture.node_at(Vec2::new(100.0, 0.0));
    let c = fixture.node_at(Vec2::new(0.0, 100.0));
    let mut document = fixture.document;
    document
        .graph
        .set_input_binding(InputPort::new(b, 0), Binding::wire(a, 0));
    document
        .graph
        .set_input_binding(InputPort::new(b, 1), Binding::Const(ConstValue::from(7i64)));
    document
        .graph
        .set_input_binding(InputPort::new(b, 2), Binding::wire(c, 0));
    document.graph.subscribe(EventPort::new(a, 0), b);
    document.main_view.selected = [a, b].map(ItemId::Node).into_iter().collect();
    CrossingWire {
        document,
        a,
        b,
        c,
        library: fixture.library,
    }
}

/// The clone placed at `origin` plus the duplicate offset — the one node
/// there that is not among `originals`.
#[track_caller]
fn clone_of(document: &Document, origin: Vec2, originals: &BTreeSet<NodeId>) -> NodeId {
    document
        .graph
        .iter()
        .map(|node| node.node_id)
        .find(|id| {
            !originals.contains(id)
                && document.main_view.node_placements[id].pos == origin + DUPLICATE_OFFSET
        })
        .unwrap_or_else(|| panic!("a clone offset from {origin:?}"))
}

/// Duplicating expands into ordinary intents — one add per clone, one
/// `SetInput` per edge *inside* the set, the internal subscriptions, then the
/// selection swap — so the whole thing commits through the same gate as any
/// other edit and undoes as one batch.
#[test]
fn duplicate_clones_wiring_and_selects_the_copies() {
    let CrossingWire {
        mut document,
        a,
        b,
        c,
        library,
    } = crossing_wire();
    let originals: BTreeSet<NodeId> = [a, b, c].into_iter().collect();

    let intents = GraphIntent::duplicate(&document, &library, IncomingWires::Drop);
    let steps = commit_all(&mut document, &library, intents);
    document
        .validate()
        .expect("a duplicate leaves a valid document");

    // Two fresh nodes, offset from their originals, and they are what is
    // selected now.
    let clones: BTreeSet<NodeId> = document
        .graph
        .iter()
        .map(|node| node.node_id)
        .filter(|id| !originals.contains(id))
        .collect();
    assert_eq!(clones.len(), 2, "both selected nodes were cloned");
    assert_eq!(
        document.main_view.selected,
        clones.iter().copied().map(ItemId::Node).collect(),
        "the copies end up selected"
    );
    let a_clone = clone_of(&document, Vec2::ZERO, &originals);
    let b_clone = clone_of(&document, Vec2::new(100.0, 0.0), &originals);

    // The internal edge is recreated against the clones, the const copies
    // verbatim, the edge from outside the set is dropped, and the internal
    // subscription is recreated.
    assert_eq!(
        document.graph.bindings.get(&InputPort::new(b_clone, 0)),
        Some(&Binding::wire(a_clone, 0)),
        "a' -> b' replaces a -> b"
    );
    assert!(matches!(
        document.graph.bindings.get(&InputPort::new(b_clone, 1)),
        Some(Binding::Const(_))
    ));
    assert_eq!(
        document.graph.bindings.get(&InputPort::new(b_clone, 2)),
        None,
        "the edge from unselected c is dropped"
    );
    assert!(
        document
            .graph
            .is_subscribed(EventPort::new(a_clone, 0), b_clone)
    );

    // One batch, so one undo: every clone and its wiring goes, and the
    // selection is the one the duplicate replaced.
    undo_all(&mut document, &steps);
    assert_eq!(document.graph.len(), 3, "only the originals are left");
    assert_eq!(
        document.main_view.selected,
        [a, b].map(ItemId::Node).into_iter().collect()
    );
    assert_eq!(
        document.graph.bindings.get(&InputPort::new(b, 2)),
        Some(&Binding::wire(c, 0)),
        "the originals' wiring was never touched"
    );
    document
        .validate()
        .expect("an undone duplicate leaves a valid document");

    // ...and redo replays it forward: each clone is attached again, then the
    // edges between them, so nothing is bound to a node that isn't back yet.
    for step in &steps {
        step.apply(&mut document);
    }
    assert_eq!(document.graph.len(), 5);
    assert_eq!(
        document.main_view.selected,
        clones.iter().copied().map(ItemId::Node).collect()
    );
    assert_eq!(
        document.graph.bindings.get(&InputPort::new(b_clone, 0)),
        Some(&Binding::wire(a_clone, 0))
    );
    document
        .validate()
        .expect("a redone duplicate leaves a valid document");
}

/// The same selection over the same crossing wire, with `IncomingWires::Keep`:
/// the clone keeps the wire from the producer outside the set — pointing at
/// the original, since that producer wasn't copied. That is the only
/// difference from the case above, where the same edge is dropped.
#[test]
fn duplicate_keeps_external_producers_on_request() {
    let CrossingWire {
        mut document,
        a,
        b,
        c,
        library,
    } = crossing_wire();
    let originals: BTreeSet<NodeId> = [a, b, c].into_iter().collect();

    let intents = GraphIntent::duplicate(&document, &library, IncomingWires::Keep);
    commit_all(&mut document, &library, intents);
    let b_clone = clone_of(&document, Vec2::new(100.0, 0.0), &originals);
    assert_eq!(
        document.graph.bindings.get(&InputPort::new(b_clone, 2)),
        Some(&Binding::wire(c, 0)),
        "the kept wire still reads the original external producer"
    );
    document.validate().unwrap();
}

#[test]
fn duplicate_of_nothing_asks_for_nothing() {
    let mut fixture = DocumentFixture::default();
    fixture.node_at(Vec2::ZERO);
    assert!(
        GraphIntent::duplicate(&fixture.document, &fixture.library, IncomingWires::Drop).is_empty()
    );
}

#[test]
fn invalid_viewports_are_dropped_before_mutation() {
    let fixture = DocumentFixture::default();
    let mut document = fixture.document;
    let initial = document.main_view.viewport;
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
            GraphIntent::SetViewport { to, gesture: None }
                .commit_one(&mut document, &fixture.library)
                .is_err(),
            "invalid viewport {to:?} must be dropped"
        );
        assert_eq!(
            document.main_view.viewport, initial,
            "an invalid viewport must not mutate the document"
        );
    }

    let valid = Viewport {
        pan: Vec2::new(10.0, -20.0),
        zoom: 2.0,
    };
    assert!(
        GraphIntent::SetViewport {
            to: valid,
            gesture: None,
        }
        .commit_one(&mut document, &fixture.library)
        .unwrap()
        .is_some(),
        "a finite positive viewport must commit"
    );
    assert_eq!(document.main_view.viewport, valid);
    document.validate().unwrap();
}

#[test]
fn subscribe_unsubscribe_commit_and_undo() {
    let mut fixture = DocumentFixture::default();
    let emitter = fixture.node_at(Vec2::ZERO);
    let subscriber = fixture.node_at(Vec2::new(100.0, 0.0));
    let mut document = fixture.document;

    // Subscribe commits and writes the edge.
    let step = subscribe(emitter, subscriber, true)
        .commit_one(&mut document, &fixture.library)
        .unwrap()
        .expect("subscribe commits");
    assert!(
        document
            .graph
            .is_subscribed(EventPort::new(emitter, 0), subscriber)
    );

    // A second identical subscribe is a no-op (from == to == true).
    assert!(
        subscribe(emitter, subscriber, true)
            .commit_one(&mut document, &fixture.library)
            .unwrap()
            .is_none(),
        "re-subscribing the same edge is a no-op"
    );

    // Undo removes it; redo restores it.
    step.revert(&mut document);
    assert!(
        !document
            .graph
            .is_subscribed(EventPort::new(emitter, 0), subscriber)
    );
    step.apply(&mut document);
    assert!(
        document
            .graph
            .is_subscribed(EventPort::new(emitter, 0), subscriber)
    );

    // Unsubscribe commits, removes the edge, and undo brings it back.
    let step = subscribe(emitter, subscriber, false)
        .commit_one(&mut document, &fixture.library)
        .unwrap()
        .expect("unsubscribe commits");
    assert!(
        !document
            .graph
            .is_subscribed(EventPort::new(emitter, 0), subscriber)
    );
    step.revert(&mut document);
    assert!(
        document
            .graph
            .is_subscribed(EventPort::new(emitter, 0), subscriber)
    );

    // Redo the unsubscribe (apply writes the `to = unsubscribed` half),
    // then unsubscribing the now-absent edge is a no-op.
    step.apply(&mut document);
    assert!(
        !document
            .graph
            .is_subscribed(EventPort::new(emitter, 0), subscriber)
    );
    assert!(
        subscribe(emitter, subscriber, false)
            .commit_one(&mut document, &fixture.library)
            .unwrap()
            .is_none(),
        "unsubscribing a missing edge is a no-op"
    );
}

#[test]
fn set_node_property_commits_and_reverts() {
    let mut fixture = DocumentFixture::default();
    let id = fixture.node_at(Vec2::ZERO);
    let mut document = fixture.document;
    // Fresh nodes default to no caching (None) and enabled.
    assert_eq!(document.graph.find(id).unwrap().cache, CacheMode::None);
    assert!(!document.graph.find(id).unwrap().disabled);

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
            .commit_one(&mut document, &fixture.library)
            .unwrap()
            .unwrap_or_else(|| panic!("{to:?} is a real change, not a no-op"));
        let node = document.graph.find(id).unwrap();
        match to {
            NodeProperty::RuntimeCache(m) => assert_eq!(node.cache, m),
            NodeProperty::Disabled(d) => assert_eq!(node.disabled, d),
        }
        assert!(
            !step.invalidates_cached_geometry(),
            "a node-property toggle does not remeasure"
        );
        step.revert(&mut document);
        let node = document.graph.find(id).unwrap();
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
                .commit_one(&mut document, &fixture.library)
                .unwrap()
                .is_none(),
            "{to:?} equals the current value → writes nothing"
        );
    }
}

#[test]
fn commit_rejects_cycle_forming_bind() {
    // a → b (b's input 0 bound to a's output 0).
    let mut fixture = DocumentFixture::default();
    let a = fixture.node_at(Vec2::ZERO);
    let b = fixture.node_at(Vec2::new(100.0, 0.0));
    let c = fixture.node_at(Vec2::new(0.0, 100.0));
    let mut document = fixture.document;
    document
        .graph
        .set_input_binding(InputPort::new(b, 0), Binding::wire(a, 0));

    // Wiring a's input back to b's output would close the a → b loop:
    // rejected, nothing written, the existing edge untouched.
    assert!(
        GraphIntent::SetInput {
            input: InputPort::new(a, 0),
            to: Some(Binding::wire(b, 0)),
        }
        .commit_one(&mut document, &fixture.library)
        .unwrap()
        .is_none(),
        "a bind that closes a cycle is rejected"
    );
    assert_eq!(
        document.graph.bindings.get(&InputPort::new(a, 0)),
        None,
        "the rejected bind left a's input unbound"
    );
    assert_eq!(
        document.graph.bindings.get(&InputPort::new(b, 0)),
        Some(&Binding::wire(a, 0)),
        "the existing a → b edge is untouched"
    );

    // A bind that keeps the graph acyclic still commits: c's input ← b's
    // output extends the chain into a → b → c.
    assert!(
        GraphIntent::SetInput {
            input: InputPort::new(c, 0),
            to: Some(Binding::wire(b, 0)),
        }
        .commit_one(&mut document, &fixture.library)
        .unwrap()
        .is_some(),
        "an acyclic bind commits"
    );
    assert_eq!(
        document.graph.bindings.get(&InputPort::new(c, 0)),
        Some(&Binding::wire(b, 0)),
    );
}

/// What leaked through a commit of `intent` expected to be refused as
/// malformed — the gate exists to keep a malformed payload out of the write
/// half — or `None` when the refusal held.
fn refusal_leak(document: &mut Document, library: &Library, intent: GraphIntent) -> Option<String> {
    let nodes = document.graph.len();
    if let Ok(outcome) = intent.commit_one(document, library) {
        return Some(format!("expected a MalformedIntent, got {outcome:?}"));
    }
    if document.graph.len() != nodes {
        return Some(format!(
            "a refused intent mutated the graph: {nodes} nodes became {}",
            document.graph.len()
        ));
    }
    document
        .validate()
        .err()
        .map(|e| format!("document invalid after a refusal: {e}"))
}

#[test]
fn insertions_reusing_an_identity_are_refused_instead_of_panicking() {
    // A live id would otherwise abort the process inside the write half, on
    // `Graph::insert`'s own duplicate-id panic, and a nil one names nothing a
    // widget could have read. Refusing at the gate states both.
    let mut fixture = DocumentFixture::default();
    let live = fixture.node_at(Vec2::ZERO);
    let stub = fixture.stub_at(Vec2::new(100.0, 0.0));
    let node = fixture.plain_node();
    let mut document = fixture.document;

    assert_eq!(
        refusal_leak(
            &mut document,
            &fixture.library,
            GraphIntent::add_node(Vec2::ZERO, stub, node.clone())
        ),
        None,
        "AddNode over a stub's id"
    );
    assert_eq!(
        refusal_leak(
            &mut document,
            &fixture.library,
            GraphIntent::add_node(Vec2::ZERO, live, node.clone())
        ),
        None,
        "AddNode over a live id"
    );
    assert_eq!(
        refusal_leak(
            &mut document,
            &fixture.library,
            GraphIntent::add_node(Vec2::ZERO, NodeId::nil(), DocumentFixture::stub_node())
        ),
        None,
        "AddNode with a nil id"
    );

    // Intents commit one at a time, so an id repeated *within* one batch is
    // refused by the same check: the second add meets a graph the first is
    // already in.
    let repeated = NodeId::unique();
    assert!(
        GraphIntent::add_node(Vec2::ZERO, repeated, node)
            .commit_one(&mut document, &fixture.library)
            .unwrap()
            .is_some()
    );
    assert_eq!(
        refusal_leak(
            &mut document,
            &fixture.library,
            GraphIntent::add_node(Vec2::ONE, repeated, DocumentFixture::stub_node())
        ),
        None,
        "a second AddNode repeating an id from the same batch"
    );
}

#[test]
fn malformed_payloads_are_refused_before_they_can_invalidate_the_document() {
    // Every case here would apply cleanly and leave a document that
    // `Document::validate` rejects, or trip an assert inside the graph — which,
    // because saving only validates in debug builds, means a project that
    // writes fine and won't reopen.
    let probe = internals::stub_func(FuncId::unique(), "probe")
        .input(FuncInput::optional("a", DataType::Int))
        .output(FuncOutput::new("out", DataType::Int))
        .build();
    let mut fixture = DocumentFixture::default();
    let live = fixture.add(&probe);
    let mut document = fixture.document;
    let ghost = NodeId::unique();
    let nan = Vec2::new(f32::NAN, 0.0);

    let nil_func = internals::unresolved_node(FuncId::nil());

    let fresh = NodeId::unique();
    let seeded = |bindings: Vec<(InputPort, Binding)>| GraphIntent::AddNode {
        pos: Vec2::ZERO,
        node_id: fresh,
        node: Node::from(&probe),
        bindings: bindings
            .into_iter()
            .map(|(port, binding)| BindingEntry { port, binding })
            .collect(),
    };
    let cases = [
        (
            "AddNode at a non-finite position",
            GraphIntent::add_node(nan, NodeId::unique(), DocumentFixture::stub_node()),
        ),
        (
            "AddNode with a nil func id",
            GraphIntent::add_node(Vec2::ZERO, NodeId::unique(), nil_func),
        ),
        (
            // The graph holds only nodes the library can run.
            "AddNode of a node whose func the library lacks",
            GraphIntent::add_node(Vec2::ZERO, NodeId::unique(), DocumentFixture::stub_node()),
        ),
        (
            "AddNode seeding a binding from a producer that isn't there",
            seeded(vec![(InputPort::new(fresh, 0), Binding::wire(ghost, 0))]),
        ),
        (
            // The insertion restores exactly the wiring it recorded, so it may
            // only author its own node's inputs.
            "AddNode seeding a binding on someone else's port",
            seeded(vec![(InputPort::new(live, 0), Binding::wire(fresh, 0))]),
        ),
        (
            // A loop of one: nothing reads the new node yet, so its own output
            // is the only source that could close a cycle — and a compile
            // refuses a cyclic graph.
            "AddNode seeding an input from the new node's own output",
            seeded(vec![(InputPort::new(fresh, 0), Binding::wire(fresh, 0))]),
        ),
        (
            // Only one could survive, so `DetachedNode::new` refuses the record.
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
            drag(&[member(live, nan)], Vec2::ZERO),
        ),
        (
            "SetSubscription carrying a nil id",
            subscribe(NodeId::nil(), live, true),
        ),
    ];
    for (what, intent) in cases {
        assert_eq!(
            refusal_leak(&mut document, &fixture.library, intent),
            None,
            "{what}"
        );
    }

    // The refusal names the field it held to, not just the check.
    let refused = |intent: GraphIntent, document: &mut Document| {
        intent
            .commit_one(document, &fixture.library)
            .expect_err("a malformed payload is refused")
    };
    assert!(matches!(
        refused(subscribe(live, NodeId::nil(), true), &mut document),
        MalformedIntent::NilNodeId {
            role: Role::SetSubscriptionSubscriber
        }
    ));
    assert!(matches!(
        refused(drag(&[member(live, nan)], Vec2::ZERO), &mut document),
        MalformedIntent::NonFinitePos {
            role: Role::MoveSelection,
            ..
        }
    ));
    assert!(matches!(
        refused(
            seeded(vec![(InputPort::new(fresh, 0), Binding::wire(ghost, 0))]),
            &mut document
        ),
        MalformedIntent::NodeAbsent {
            role: Role::BindingProducer,
            node_id,
        } if node_id == ghost
    ));
    assert_eq!(
        Role::SetInputProducer.to_string(),
        "SetInput producer",
        "a role reads as the intent and field it names"
    );
}

#[test]
fn stale_references_still_refuse_quietly() {
    // Widgets emit identities they read out of the live document, so the
    // only thing they get wrong is staleness — an anchor removed between
    // the gesture starting and the intent draining. Those stay silent;
    // turning them into reported failures would spam the status bar on
    // ordinary use.
    let mut fixture = DocumentFixture::default();
    let live = fixture.node_at(Vec2::ZERO);
    let mut document = fixture.document;
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
        ("Raise", GraphIntent::Raise { node_id: gone }),
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
                to: Some(Binding::wire(gone, 0)),
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
            drag(&[member(gone, Vec2::ZERO)], Vec2::ZERO),
        ),
    ];
    // The silent drop widgets rely on is an `Ok` outcome, not an error.
    for (what, intent) in cases {
        match intent.commit_one(&mut document, &fixture.library) {
            Ok(None) => {}
            other => panic!("{what}: expected no step, got {other:?}"),
        }
    }
    assert!(document.graph.bindings.is_empty(), "nothing was written");
    document.validate().expect("document stays valid");
}

#[test]
fn selection_and_move_drop_members_whose_widget_is_gone() {
    // The rubber band and a group drag both snapshot identities when the
    // gesture starts, and undo runs before the gesture prepass — so a
    // member can disappear mid-gesture. Recording it verbatim would leave
    // `selected` naming an item the view can't render, which is exactly
    // `GraphViewValidationError::MissingSelectedItem`.
    let mut fixture = DocumentFixture::default();
    let live = fixture.node_at(Vec2::ZERO);
    let mut document = fixture.document;
    let gone = NodeId::unique();

    let step = GraphIntent::SetSelection {
        to: [
            ItemId::Node(live),
            ItemId::Node(gone),
            ItemId::Tie(TieId::unique()),
        ]
        .into_iter()
        .collect(),
    }
    .commit_one(&mut document, &fixture.library)
    .unwrap()
    .expect("a selection with one live member commits");
    let UndoStep::Selection(step) = &step else {
        panic!("expected a SetSelection step, got {step:?}");
    };
    assert_eq!(
        step.selection.to,
        [ItemId::Node(live)].into_iter().collect::<BTreeSet<_>>(),
        "the vanished node and the tie no view holds are dropped, the live one kept"
    );
    assert_eq!(document.main_view.selected, step.selection.to);

    // `live` latched at (1, 2) and sits at the origin now: the step moves it
    // from where it sits to (1, 2) + (4, 4) = (5, 6).
    let step = drag(
        &[member(live, Vec2::new(1.0, 2.0)), member(gone, Vec2::ZERO)],
        Vec2::new(4.0, 4.0),
    )
    .commit_one(&mut document, &fixture.library)
    .unwrap()
    .expect("a move with one live member commits");
    let UndoStep::Positions(step) = &step else {
        panic!("expected a SetPositions step, got {step:?}");
    };
    assert_eq!(step.moves.len(), 1, "only the surviving member is recorded");
    assert_eq!(step.moves[0].item, ItemId::Node(live));
    assert_eq!(
        (step.moves[0].pos.from, step.moves[0].pos.to),
        (Vec2::ZERO, Vec2::new(5.0, 6.0))
    );
    document.validate().expect("document stays valid");
}

/// A wire onto an input its func declares const-only is refused at the gate,
/// on an existing node and as an insertion's seed alike — the canvas never
/// offers one, and a graph holding it fails every compile. A constant on the
/// same input commits.
#[test]
fn a_wire_onto_a_const_only_input_is_refused() {
    let knob_func = internals::stub_func(FuncId::unique(), "knob")
        .input(FuncInput::required("strength", DataType::Float).const_only())
        .build();
    let feed_func = internals::stub_func(FuncId::unique(), "feed")
        .output(FuncOutput::new("value", DataType::Float))
        .build();
    let mut fixture = DocumentFixture::default();
    let knob = fixture.add(&knob_func);
    let feed = fixture.add(&feed_func);
    let mut document = fixture.document;
    let input = InputPort::new(knob, 0);

    let wired = GraphIntent::SetInput {
        input,
        to: Some(Binding::wire(feed, 0)),
    }
    .commit_one(&mut document, &fixture.library);
    assert!(
        matches!(
            wired,
            Err(MalformedIntent::Rejected { port, rejection: BindingRejection::WiredConstOnly })
                if port == input
        ),
        "{wired:?}"
    );
    assert!(
        document.graph.bindings.is_empty(),
        "the refusal wrote nothing"
    );

    let fresh = NodeId::unique();
    let seeded = GraphIntent::AddNode {
        pos: Vec2::ZERO,
        node_id: fresh,
        node: Node::from(&knob_func),
        bindings: vec![BindingEntry {
            port: InputPort::new(fresh, 0),
            binding: Binding::wire(feed, 0),
        }],
    }
    .commit_one(&mut document, &fixture.library);
    assert!(
        matches!(
            seeded,
            Err(MalformedIntent::Rejected { port, rejection: BindingRejection::WiredConstOnly })
                if port.node_id == fresh
        ),
        "{seeded:?}"
    );
    assert!(
        document.graph.find(fresh).is_none(),
        "the refused node was not added"
    );

    let constant = GraphIntent::SetInput {
        input,
        to: Some(Binding::Const(ConstValue::Float(0.5))),
    }
    .commit_one(&mut document, &fixture.library);
    assert!(matches!(constant, Ok(Some(_))), "{constant:?}");
}

/// Every index an intent writes is held to the node's declaration: the probe
/// declares one input, one output and one event, so index 0 of each commits
/// and index 1 — the first past the end — is refused, on an existing node and
/// on an insertion's seed alike. Clearing an undeclared input is not a write,
/// so it is not held to anything.
#[test]
fn indices_past_the_declaration_are_refused() {
    let probe = internals::stub_func(FuncId::unique(), "probe")
        .input(FuncInput::optional("a", DataType::Int))
        .output(FuncOutput::new("out", DataType::Int))
        .event("tick", internals::stub_event())
        .build();
    let mut fixture = DocumentFixture::default();
    let (producer, consumer) = (fixture.add(&probe), fixture.add(&probe));
    let mut document = fixture.document;
    let library = &fixture.library;
    let constant = || Some(Binding::Const(ConstValue::from(1i64)));
    let set = |port_idx: usize, to: Option<Binding>| GraphIntent::SetInput {
        input: InputPort::new(consumer, port_idx),
        to,
    };
    let fresh = NodeId::unique();
    let seeded = |port_idx: usize, binding: Binding| GraphIntent::AddNode {
        pos: Vec2::ZERO,
        node_id: fresh,
        node: Node::from(&probe),
        bindings: vec![BindingEntry {
            port: InputPort::new(fresh, port_idx),
            binding,
        }],
    };
    let listen = |event_idx: usize| GraphIntent::SetSubscription {
        subscription: Subscription::new(EventPort::new(producer, event_idx), consumer),
        subscribe: true,
    };

    let refusals = [
        ("a constant on input 1", set(1, constant())),
        (
            "a wire from output 1",
            set(0, Some(Binding::wire(producer, 1))),
        ),
        (
            "a seed constant on input 1",
            seeded(1, Binding::Const(ConstValue::from(1i64))),
        ),
        (
            "a seed wire from output 1",
            seeded(0, Binding::wire(producer, 1)),
        ),
        ("a subscription to event 1", listen(1)),
    ];
    for (what, intent) in refusals {
        assert_eq!(refusal_leak(&mut document, library, intent), None, "{what}");
    }

    let refused = |intent: GraphIntent, document: &mut Document| {
        intent
            .commit_one(document, library)
            .expect_err("an undeclared index is refused")
    };
    assert!(matches!(
        refused(set(1, constant()), &mut document),
        MalformedIntent::Rejected { port, rejection: BindingRejection::UndeclaredInput }
            if port == InputPort::new(consumer, 1)
    ));
    assert!(matches!(
        refused(set(0, Some(Binding::wire(producer, 1))), &mut document),
        MalformedIntent::Rejected { port, rejection: BindingRejection::UndeclaredOutput }
            if port == InputPort::new(consumer, 0)
    ));
    assert!(matches!(
        refused(listen(1), &mut document),
        MalformedIntent::RejectedSubscription {
            subscription,
            rejection: SubscriptionRejection::UndeclaredEvent,
        } if subscription.event.event_idx == 1
    ));

    assert!(
        matches!(set(1, None).commit_one(&mut document, library), Ok(None)),
        "clearing an undeclared input writes nothing and refuses nothing"
    );
    for (what, intent) in [
        ("a constant on input 0", set(0, constant())),
        (
            "a wire from output 0",
            set(0, Some(Binding::wire(producer, 0))),
        ),
        ("a subscription to event 0", listen(0)),
        (
            "a seed wire from output 0",
            seeded(0, Binding::wire(producer, 0)),
        ),
    ] {
        let committed = intent.commit_one(&mut document, library);
        assert!(matches!(committed, Ok(Some(_))), "{what}: {committed:?}");
    }
}

/// What an edit case names, the edit, and what it checks of the document
/// after it.
type Edit<'a> = (&'a str, GraphIntent, &'a dyn Fn(&Document));

/// Three plain nodes in a row and four wires: `ab = a.0 → b.0`, `ac = a.0 →
/// c.0`, `bc = b.0 → c.1` and the event wire `tick = a.tick → b`. The tie
/// `held` holds `ab` and `tick` and is selected; the tie `shared` holds `ac`
/// and `bc`.
struct Tied {
    document: Document,
    library: Library,
    nodes: [NodeId; 3],
    ab: WireKey,
    ac: WireKey,
    bc: WireKey,
    tick: WireKey,
    held: TieId,
    shared: TieId,
}

fn tied() -> Tied {
    let mut fixture = DocumentFixture::default();
    let nodes @ [a, b, c] = [0.0, 220.0, 440.0].map(|x| fixture.node_at(Vec2::new(x, 0.0)));
    let ab = fixture.wire(OutputPort::new(a, 0), InputPort::new(b, 0));
    let ac = fixture.wire(OutputPort::new(a, 0), InputPort::new(c, 0));
    let bc = fixture.wire(OutputPort::new(b, 0), InputPort::new(c, 1));
    let tick = fixture.subscribe(EventPort::new(a, 0), b);
    let held = fixture.tie(Vec2::new(110.0, 20.0), [ab, tick]);
    let shared = fixture.tie(Vec2::new(330.0, 20.0), [ac, bc]);
    let DocumentFixture {
        mut document,
        library,
    } = fixture;
    document.main_view.selected.insert(ItemId::Tie(held));
    document.validate().unwrap();
    Tied {
        document,
        library,
        nodes,
        ab,
        ac,
        bc,
        tick,
        held,
        shared,
    }
}

/// Each tie edit lands as one `SetTies` step, a tie's move as a
/// `SetPositions`, and each leaves the view as the case says; a revert puts
/// the document back exactly. A tie edit dirties the document and leaves the
/// node set and the geometry caches alone.
#[test]
fn tie_edits_apply_and_revert() {
    let Tied {
        document,
        library,
        ab,
        ac,
        bc,
        tick,
        held,
        shared,
        ..
    } = tied();
    let fresh = TieId::unique();
    let route = |ties: &[TieId]| Some(Route::new(ties.iter().copied()));
    let cases: [Edit<'_>; 7] = [
        (
            "TieWires puts a new tie on each route at its crossed piece",
            GraphIntent::TieWires {
                tie: fresh,
                pos: Vec2::new(50.0, 60.0),
                wires: vec![
                    RouteInsert { wire: ac, at: 1 },
                    RouteInsert { wire: ab, at: 0 },
                ],
            },
            &|document| {
                let view = &document.main_view;
                assert_eq!(
                    view.ties[&fresh],
                    Tie {
                        pos: Vec2::new(50.0, 60.0),
                        angle: TieAngle::Auto
                    }
                );
                assert_eq!(view.routes.get(&ac).cloned(), route(&[shared, fresh]));
                assert_eq!(view.routes.get(&ab).cloned(), route(&[fresh, held]));
            },
        ),
        (
            "JoinTie adds a wire to a tie",
            GraphIntent::JoinTie {
                tie: held,
                wires: vec![RouteInsert { wire: ac, at: 0 }],
            },
            &|document| {
                assert_eq!(
                    document.main_view.routes.get(&ac).cloned(),
                    route(&[held, shared])
                );
            },
        ),
        (
            "RemoveTie removes the tie, its wires' steps through it, and its selection",
            GraphIntent::RemoveTie { tie: held },
            &|document| {
                let view = &document.main_view;
                assert!(!view.ties.contains_key(&held));
                assert_eq!(
                    view.routes.keys().copied().collect::<BTreeSet<_>>(),
                    BTreeSet::from([ac, bc])
                );
                assert!(view.selected.is_empty());
            },
        ),
        (
            "SetTieAngle pins the band",
            GraphIntent::SetTieAngle {
                tie: held,
                angle: TieAngle::Pinned(45.0),
                gesture: None,
            },
            &|document| {
                assert_eq!(document.main_view.ties[&held].angle, TieAngle::Pinned(45.0));
            },
        ),
        (
            "SetTieAngle frees a pinned band",
            GraphIntent::SetTieAngle {
                tie: shared,
                angle: TieAngle::Auto,
                gesture: None,
            },
            &|document| {
                assert_eq!(document.main_view.ties[&shared].angle, TieAngle::Auto);
            },
        ),
        (
            "ReleaseWire takes one wire out and keeps the tie that holds another",
            GraphIntent::ReleaseWire {
                tie: held,
                wire: tick,
            },
            &|document| {
                let view = &document.main_view;
                assert!(!view.routes.contains_key(&tick));
                assert_eq!(view.routes.get(&ab).cloned(), route(&[held]));
                assert!(view.ties.contains_key(&held));
            },
        ),
        (
            "a drag frame moves a tie",
            drag(
                &[DragStart {
                    item: ItemId::Tie(held),
                    pos: Vec2::new(110.0, 20.0),
                }],
                Vec2::new(5.0, -5.0),
            ),
            &|document| {
                assert_eq!(document.main_view.ties[&held].pos, Vec2::new(115.0, 15.0));
            },
        ),
    ];
    for (what, intent, check) in cases {
        let mut document = document.clone();
        // A pinned angle to free, so freeing it is not a no-op.
        document.main_view.ties.get_mut(&shared).unwrap().angle = TieAngle::Pinned(10.0);
        let before = document.clone();
        let step = intent
            .commit_one(&mut document, &library)
            .unwrap()
            .unwrap_or_else(|| panic!("{what}: no step"));
        if let UndoStep::Ties(_) = step {
            assert!(step.dirties_document(), "{what}");
            assert!(!step.invalidates_cached_geometry(), "{what}");
            assert!(!step.changes_node_set(), "{what}");
        }
        check(&document);
        document
            .validate()
            .unwrap_or_else(|error| panic!("{what}: {error}"));
        step.revert(&mut document);
        assert_eq!(document, before, "{what}: the revert puts it back");
    }
}

/// Releasing a tie's last wire removes the tie with it and takes it out of the
/// selection; the reverts, in reverse, bring the wire, the tie and its
/// selection back.
#[test]
fn releasing_a_ties_last_wire_removes_the_tie() {
    let Tied {
        mut document,
        library,
        ab,
        tick,
        held,
        ..
    } = tied();
    let before = document.clone();
    let steps = commit_all(
        &mut document,
        &library,
        [
            GraphIntent::ReleaseWire {
                tie: held,
                wire: tick,
            },
            GraphIntent::ReleaseWire {
                tie: held,
                wire: ab,
            },
        ],
    );
    assert_eq!(steps.len(), 2);
    assert!(!document.main_view.ties.contains_key(&held));
    assert!(!document.main_view.selected.contains(&ItemId::Tie(held)));
    document.validate().unwrap();
    undo_all(&mut document, &steps);
    assert_eq!(document, before);
}

/// A wire is both of its ends: rewiring `c.0` from `a` to `b` drops the route
/// of `ac`, while `shared` stays for `bc`; cutting `b.0` drops the route of
/// `ab`, while `held` stays for `tick`; unsubscribing
/// `tick` drops its route. A bind to the same output changes nothing, so it
/// records no step and keeps the route. Removing `a` ends `ab`, `ac` and
/// `tick`: `held`, which only they held, goes, and `shared` stays for `bc`.
/// Each revert brings all of it back.
#[test]
fn wiring_edits_take_the_routes_of_the_wires_they_end() {
    let Tied {
        document,
        library,
        nodes: [a, b, c],
        ab,
        ac,
        bc,
        tick,
        held,
        shared,
    } = tied();
    let input = InputPort::new(b, 0);
    let mut same = document.clone();
    assert!(
        GraphIntent::set_input(input, Binding::wire(a, 0))
            .commit_one(&mut same, &library)
            .unwrap()
            .is_none(),
        "a bind to the same output is no edit"
    );
    assert_eq!(same, document, "and keeps the route");

    let cases: [Edit<'_>; 4] = [
        (
            "a rewire to another output",
            GraphIntent::set_input(InputPort::new(c, 0), Binding::wire(b, 0)),
            &|document| {
                assert!(!document.main_view.routes.contains_key(&ac));
                assert!(document.main_view.ties.contains_key(&shared));
            },
        ),
        ("a cut", GraphIntent::set_input(input, None), &|document| {
            assert!(!document.main_view.routes.contains_key(&ab));
            assert!(document.main_view.ties.contains_key(&held));
        }),
        ("an unsubscribe", subscribe(a, b, false), &|document| {
            assert!(!document.main_view.routes.contains_key(&tick));
            assert!(document.main_view.routes.contains_key(&ab));
        }),
        (
            "a removal of the node every wire of `held` touches",
            GraphIntent::RemoveNode { node_id: a },
            &|document| {
                let view = &document.main_view;
                assert!(!view.ties.contains_key(&held), "`held` goes");
                assert!(view.selected.is_empty(), "and leaves the selection");
                assert_eq!(
                    view.routes.iter().collect::<Vec<_>>(),
                    [(&bc, &Route::new([shared]))],
                    "`shared` keeps `bc` alone"
                );
            },
        ),
    ];
    for (what, intent, check) in cases {
        let mut document = document.clone();
        let before = document.clone();
        let steps = commit_all(&mut document, &library, [intent]);
        check(&document);
        document
            .validate()
            .unwrap_or_else(|error| panic!("{what}: {error}"));
        undo_all(&mut document, &steps);
        assert_eq!(document, before, "{what}: the revert puts it back");
    }
}

/// Delete removes the selected nodes, then the selected ties. A tie the node
/// removal took with it is stale by its turn, so the batch commits one step.
#[test]
fn delete_removes_the_selected_ties_after_the_nodes() {
    let Tied {
        mut document,
        library,
        nodes: [a, ..],
        held,
        shared,
        ..
    } = tied();
    document.main_view.selected = [ItemId::Node(a), ItemId::Tie(held), ItemId::Tie(shared)]
        .into_iter()
        .collect();
    let intents: Vec<GraphIntent> = GraphIntent::remove_selection(&document.main_view).collect();
    let mut ties = [held, shared];
    ties.sort_unstable();
    assert!(
        matches!(
            intents[..],
            [
                GraphIntent::RemoveNode { node_id },
                GraphIntent::RemoveTie { tie: first },
                GraphIntent::RemoveTie { tie: second },
            ] if node_id == a && [first, second] == ties
        ),
        "{intents:?}"
    );
    let steps = commit_all(&mut document, &library, intents);
    assert_eq!(
        steps.len(),
        2,
        "the node's removal, and `shared`'s: `held` went with the node"
    );
    assert!(document.main_view.ties.is_empty());
    document.validate().unwrap();
}

/// A tie edit that could never apply is refused with its case, and one whose
/// tie went since the gesture started gives no step.
#[test]
fn tie_intents_refuse_malformed_payloads_and_drop_stale_ones() {
    let Tied {
        mut document,
        library,
        nodes: [a, _, c],
        ab,
        held,
        ..
    } = tied();
    let unheld = WireKey::Event(Subscription::new(EventPort::new(c, 0), a));
    let fresh = TieId::unique();
    let tie_wires = |tie, wires| GraphIntent::TieWires {
        tie,
        pos: Vec2::ZERO,
        wires,
    };
    let refused = [
        (
            "an id the view holds",
            tie_wires(held, vec![RouteInsert { wire: ab, at: 0 }]),
            MalformedIntent::TieAlreadyExists { tie: held },
        ),
        (
            "no wire",
            tie_wires(fresh, Vec::new()),
            MalformedIntent::EmptyTie { tie: fresh },
        ),
        (
            "a wire the graph does not hold",
            tie_wires(
                fresh,
                vec![RouteInsert {
                    wire: unheld,
                    at: 0,
                }],
            ),
            MalformedIntent::WireAbsent { wire: unheld },
        ),
        (
            "a piece past the route's end",
            tie_wires(fresh, vec![RouteInsert { wire: ab, at: 2 }]),
            MalformedIntent::RouteIndexOutOfRange {
                wire: ab,
                at: 2,
                len: 1,
            },
        ),
        (
            "one wire crossed twice",
            tie_wires(
                fresh,
                vec![
                    RouteInsert { wire: ab, at: 0 },
                    RouteInsert { wire: ab, at: 0 },
                ],
            ),
            MalformedIntent::TieAlreadyOnRoute {
                wire: ab,
                tie: fresh,
            },
        ),
        (
            "a join of a wire the tie holds",
            GraphIntent::JoinTie {
                tie: held,
                wires: vec![RouteInsert { wire: ab, at: 1 }],
            },
            MalformedIntent::TieAlreadyOnRoute {
                wire: ab,
                tie: held,
            },
        ),
        (
            "an angle of 360",
            GraphIntent::SetTieAngle {
                tie: held,
                angle: TieAngle::Pinned(360.0),
                gesture: None,
            },
            MalformedIntent::InvalidTieAngle {
                tie: held,
                degrees: 360.0,
            },
        ),
        (
            "a position that is not finite",
            GraphIntent::TieWires {
                tie: fresh,
                pos: Vec2::NAN,
                wires: vec![RouteInsert { wire: ab, at: 0 }],
            },
            MalformedIntent::NonFinitePos {
                role: Role::TieWires,
                pos: Vec2::NAN,
            },
        ),
    ];
    let before = document.clone();
    for (what, intent, expected) in refused {
        let result = intent.commit_one(&mut document, &library);
        // Debug, as `MalformedIntent` carries foreign errors with no equality.
        assert_eq!(
            format!("{:?}", result.map(|_| ())),
            format!("{:?}", Err::<(), _>(expected)),
            "{what}"
        );
        assert_eq!(document, before, "{what}: nothing is written");
    }

    let gone = TieId::unique();
    let stale = [
        GraphIntent::JoinTie {
            tie: gone,
            wires: vec![RouteInsert { wire: ab, at: 0 }],
        },
        GraphIntent::RemoveTie { tie: gone },
        GraphIntent::SetTieAngle {
            tie: gone,
            angle: TieAngle::Auto,
            gesture: None,
        },
        GraphIntent::ReleaseWire {
            tie: gone,
            wire: ab,
        },
        GraphIntent::ReleaseWire {
            tie: held,
            wire: unheld,
        },
    ];
    for intent in stale {
        let what = format!("{intent:?}");
        assert!(
            intent
                .commit_one(&mut document, &library)
                .unwrap()
                .is_none(),
            "{what}"
        );
    }
    assert_eq!(document, before);
}

/// A duplicate of `a` and `b` takes the ties inside them, at the offset and
/// with their angles, and the cloned wire runs through the clones in its
/// original's order: `ab` runs through `second` then `first`, so `ab'` runs
/// through `second'` then `first'`. `mixed` also holds `cb`, which comes from
/// `c` outside the set, so even with `IncomingWires::Keep`, which keeps `cb'`,
/// it is not cloned, and `cb'` has no route. The clones end up selected with
/// the copied nodes, and one undo takes all of it back.
#[test]
fn duplicate_takes_the_ties_inside_the_set() {
    let mut fixture = DocumentFixture::default();
    let [a, b, c] = [0.0, 220.0, 440.0].map(|x| fixture.node_at(Vec2::new(x, 0.0)));
    let ab = fixture.wire(OutputPort::new(a, 0), InputPort::new(b, 0));
    let cb = fixture.wire(OutputPort::new(c, 0), InputPort::new(b, 1));
    let first = fixture.tie(Vec2::new(150.0, 0.0), [ab]);
    let second = fixture.tie(Vec2::new(60.0, 0.0), [ab]);
    let mixed = fixture.tie(Vec2::new(300.0, 0.0), [cb]);
    let DocumentFixture {
        mut document,
        library,
    } = fixture;
    let view = &mut document.main_view;
    view.routes.insert(ab, Route::new([second, first, mixed]));
    view.ties.get_mut(&first).unwrap().angle = TieAngle::Pinned(30.0);
    view.selected = [ItemId::Node(a), ItemId::Node(b)].into_iter().collect();
    document.validate().unwrap();
    let before = document.clone();

    let intents = GraphIntent::duplicate(&document, &library, IncomingWires::Keep);
    let steps = commit_all(&mut document, &library, intents);
    document.validate().unwrap();
    let originals = BTreeSet::from([a, b, c]);
    let a_clone = clone_of(&document, Vec2::ZERO, &originals);
    let b_clone = clone_of(&document, Vec2::new(220.0, 0.0), &originals);
    let view = &document.main_view;
    let clone_at = |pos: Vec2| {
        *view
            .ties
            .iter()
            .find(|(tie, placed)| ![first, second, mixed].contains(tie) && placed.pos == pos)
            .unwrap()
            .0
    };
    let first_clone = clone_at(Vec2::new(150.0, 0.0) + DUPLICATE_OFFSET);
    let second_clone = clone_at(Vec2::new(60.0, 0.0) + DUPLICATE_OFFSET);
    assert_eq!(view.ties.len(), 5, "`mixed` is not cloned");
    assert_eq!(view.ties[&first_clone].angle, TieAngle::Pinned(30.0));
    assert_eq!(view.ties[&second_clone].angle, TieAngle::Auto);
    let ab_clone = WireKey::Data(DataWire {
        output: OutputPort::new(a_clone, 0),
        input: InputPort::new(b_clone, 0),
    });
    let cb_clone = WireKey::Data(DataWire {
        output: OutputPort::new(c, 0),
        input: InputPort::new(b_clone, 1),
    });
    assert!(
        cb_clone.is_held_by(&document.graph),
        "the kept incoming wire"
    );
    assert_eq!(
        view.routes[&ab_clone],
        Route::new([second_clone, first_clone])
    );
    assert!(!view.routes.contains_key(&cb_clone));
    assert_eq!(
        view.selected,
        [a_clone, b_clone]
            .map(ItemId::Node)
            .into_iter()
            .chain([first_clone, second_clone].map(ItemId::Tie))
            .collect()
    );

    undo_all(&mut document, &steps);
    assert_eq!(document, before);
}

/// The frames of one knob drag fold into one step: it keeps the angle before the drag and takes
/// the last frame's, so one revert puts the band back where the drag found it.
#[test]
fn a_knob_drags_frames_fold_into_one_step() {
    let Tied {
        mut document,
        library,
        held,
        ..
    } = tied();
    let before = document.clone();
    let gesture = Some(GestureId::default().next());
    let turn = |degrees| GraphIntent::SetTieAngle {
        tie: held,
        angle: TieAngle::Pinned(degrees),
        gesture,
    };
    let mut open = turn(10.0)
        .commit_one(&mut document, &library)
        .unwrap()
        .unwrap();
    let next = turn(20.0)
        .commit_one(&mut document, &library)
        .unwrap()
        .unwrap();
    open.absorb(&next);
    assert_eq!(document.main_view.ties[&held].angle, TieAngle::Pinned(20.0));
    open.revert(&mut document);
    assert_eq!(document, before);
    open.apply(&mut document);
    assert_eq!(document.main_view.ties[&held].angle, TieAngle::Pinned(20.0));
}
