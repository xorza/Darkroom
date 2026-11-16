use std::collections::BTreeSet;

use glam::Vec2;
use scenarium::{
    Binding, BindingEntry, BindingRejection, Boundary, CacheMode, ConstValue, DataType,
    DetachedNodeError, EventPort, FuncId, GhostBinding, GraphId, InputDecl, InputPort, InputState,
    Interface, Library, Node, NodeId, OutputDecl, OutputPort, PlacementRejection, ProgramShape,
    Subgraph, SubgraphId, Subscription, SubscriptionRejection, Unresolved, internals,
};
use scenarium::{PortId, internals::test_graph::TestGraph};

use crate::core::document::Document;
use crate::core::document::internals::{DocumentFixture, insert_subgraph};
use crate::core::document::item_id::ItemId;
use crate::core::document::route::Route;
use crate::core::document::tie::{Tie, TieAngle, TieId};
use crate::core::document::viewport::Viewport;
use crate::core::document::wire_key::{DataWire, WireKey};
use crate::core::edit::edit_facts::{EditFacts, Touched};
use crate::core::edit::error::{EditRefusal, IntentError, MalformedIntent, Role};
use crate::core::edit::gesture_id::GestureId;
use crate::core::edit::graph_intent::click_mode::ClickMode;
use crate::core::edit::graph_intent::drag_start::DragStart;
use crate::core::edit::graph_intent::incoming_wires::IncomingWires;
use crate::core::edit::graph_intent::node_property::NodeProperty;
use crate::core::edit::graph_intent::route_insert::RouteInsert;
use crate::core::edit::graph_intent::{DUPLICATE_OFFSET, GraphIntent};
use crate::core::edit::step::graph_step::GraphStep;
use crate::core::edit::step::undo_step::UndoStep;

/// The facts of an edit of the main graph that changes no shape.
fn main_touched() -> EditFacts {
    EditFacts {
        touched: Touched::Graph(GraphId::Main),
        ..EditFacts::NONE
    }
}

/// A duplicate's id source in a test: the next of a run of ids no fixture mints.
fn counter() -> impl FnMut() -> u128 {
    let mut last = 1_u128 << 100;
    move || {
        last += 1;
        last
    }
}

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

/// Commit a list of intents as one frame's drain does, each against the document the one before
/// left, and hand back the steps in the order applied.
#[track_caller]
fn commit_all(
    document: &mut Document,
    library: &Library,
    intents: impl IntoIterator<Item = GraphIntent>,
) -> Vec<UndoStep> {
    let mut steps = Vec::new();
    for intent in intents {
        intent
            .commit(document, GraphId::Main, library, &mut steps)
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

fn subscribe(emitter: NodeId, subscriber: NodeId, to: bool) -> GraphIntent {
    GraphIntent::SetSubscription {
        subscription: Subscription::new(EventPort::new(emitter, PortId::new(1)), subscriber),
        to,
    }
}

/// A plain click always raises its node; a Shift-click that *removes* a node must not. A tie has
/// no depth, so a click on one only selects.
#[test]
fn click_raises_unless_shift_deselects() {
    let (a, b) = (NodeId::unique(), NodeId::unique());
    let tie = ItemId::Tie(TieId::unique());
    let (na, nb) = (ItemId::Node(a), ItemId::Node(b));
    // What a click raises: the selection it sets, and the node it raises, if any.
    let click = |mode, selected: &[ItemId], item| -> (BTreeSet<ItemId>, Option<NodeId>) {
        let selected: BTreeSet<ItemId> = selected.iter().copied().collect();
        match &*GraphIntent::click(mode, &selected, item).collect::<Vec<_>>() {
            [GraphIntent::SetSelection { to }] => (to.clone(), None),
            [
                GraphIntent::SetSelection { to },
                GraphIntent::Raise { node_id },
            ] => (to.clone(), Some(*node_id)),
            other => panic!("{other:?}"),
        }
    };
    let set = |items: &[ItemId]| items.iter().copied().collect::<BTreeSet<_>>();
    for (what, mode, selected, item, expected) in [
        (
            "a plain click selects and raises",
            ClickMode::Replace,
            &[][..],
            na,
            (set(&[na]), Some(a)),
        ),
        (
            "an already-selected node still raises",
            ClickMode::Replace,
            &[na],
            na,
            (set(&[na]), Some(a)),
        ),
        (
            "a shift-click adding a node raises it",
            ClickMode::Toggle,
            &[na],
            nb,
            (set(&[na, nb]), Some(b)),
        ),
        (
            "a shift-click removing one does not",
            ClickMode::Toggle,
            &[na, nb],
            nb,
            (set(&[na]), None),
        ),
        (
            "a click on a tie selects it alone, raising nothing",
            ClickMode::Replace,
            &[na],
            tie,
            (set(&[tie]), None),
        ),
        (
            "a shift-click on a tie adds it",
            ClickMode::Toggle,
            &[na],
            tie,
            (set(&[na, tie]), None),
        ),
    ] {
        assert_eq!(click(mode, selected, item), expected, "{what}");
    }
}

/// Undoing a node removal puts back state from three places: the graph record with every edge
/// that touched the node, its placement, and its selection membership, which lets the step
/// assert its own removal against them. A ghost node comes back the same way, to the ghost nodes.
#[test]
fn a_removed_node_comes_back_whole() {
    let mut fixture = DocumentFixture::default();
    let a = fixture.node_at(Vec2::new(10.0, 20.0));
    let b = fixture.node_at(Vec2::new(100.0, 0.0));
    let ghost = fixture.ghost_node_at(Vec2::new(0.0, 100.0));
    let mut document = fixture.document;
    let edge = InputPort::new(b, PortId::new(1));
    document
        .graphs
        .main_mut()
        .set_input_binding(edge, Binding::wire(a, PortId::new(1)));
    document
        .graphs
        .main_mut()
        .subscribe(EventPort::new(a, PortId::new(1)), b);
    document.main_view.selected = [a].map(ItemId::Node).into_iter().collect();
    let placement = document.main_view.node_placements[&a];

    let step = GraphIntent::RemoveNode { node_id: a }
        .commit_one(&mut document, &fixture.library)
        .unwrap()
        .expect("removing a live node commits");
    assert!(document.graphs.main().find(a).is_none());
    assert!(!document.main_view.node_placements.contains_key(&a));
    assert!(
        !document.main_view.selected.contains(&ItemId::Node(a)),
        "selection is pruned"
    );
    assert_eq!(
        document.graphs.main().bindings.get(&edge),
        None,
        "the edge went with it"
    );
    assert!(
        !document
            .graphs
            .main()
            .is_subscribed(EventPort::new(a, PortId::new(1)), b),
        "so did the subscription"
    );
    document
        .validate()
        .expect("a removal leaves a valid document");

    step.revert(&mut document);
    assert!(document.graphs.main().find(a).is_some());
    assert_eq!(
        document.main_view.node_placements[&a], placement,
        "position *and* paint depth come back, not a fresh frontmost slot"
    );
    assert!(document.main_view.selected.contains(&ItemId::Node(a)));
    assert_eq!(
        document.graphs.main().bindings.get(&edge),
        Some(&Binding::wire(a, PortId::new(1)))
    );
    assert!(
        document
            .graphs
            .main()
            .is_subscribed(EventPort::new(a, PortId::new(1)), b)
    );
    document
        .validate()
        .expect("an undone removal leaves a valid document");

    // Redo takes it out again; the step checks what came out against what it restored.
    step.apply(&mut document);
    assert!(document.graphs.main().find(a).is_none());
    assert_eq!(document.graphs.main().bindings.get(&edge), None);

    let record = document.graphs.main().snapshot_ghost_node(ghost).unwrap();
    let ghost_placement = document.main_view.node_placements[&ghost];
    let step = GraphIntent::RemoveNode { node_id: ghost }
        .commit_one(&mut document, &fixture.library)
        .unwrap()
        .expect("removing a ghost node commits");
    assert!(document.graphs.main().ghost_node(ghost).is_none());
    assert!(!document.main_view.node_placements.contains_key(&ghost));
    document
        .validate()
        .expect("a ghost node's removal leaves a valid document");
    step.revert(&mut document);
    assert_eq!(
        document.graphs.main().snapshot_ghost_node(ghost),
        Some(record)
    );
    assert_eq!(document.main_view.node_placements[&ghost], ghost_placement);
    assert!(
        document.graphs.main().find(ghost).is_none(),
        "a ghost node stays out of the graph"
    );
    document
        .validate()
        .expect("an undone ghost node removal leaves a valid document");
}

/// An edit takes a ghost and never makes one: a wire bound onto an input a ghost wire lands on
/// replaces the ghost, clearing an input clears its ghost, on a ghost node's input too, and a
/// removed node, live or ghost, takes the ghosts that touch it. Undo puts each back as it was.
#[test]
fn an_edit_takes_a_ghost_and_its_undo_puts_it_back() {
    let mut fixture = DocumentFixture::default();
    let a = fixture.node_at(Vec2::new(10.0, 20.0));
    let b = fixture.node_at(Vec2::new(100.0, 0.0));
    let ghost = fixture.ghost_node_at(Vec2::new(0.0, 100.0));
    let library = fixture.library;
    let mut document = fixture.document;
    let into_b = InputPort::new(b, PortId::new(1));
    let into_ghost = InputPort::new(ghost, PortId::new(1));
    let from_a = Binding::wire(a, PortId::new(1));
    let kept = |binding: Binding| InputState::Ghost(GhostBinding::from(binding));
    {
        let graph = document.graphs.main_mut();
        graph.set_input_state(into_b, kept(Binding::wire(ghost, PortId::new(1))));
        graph.set_input_state(into_ghost, kept(from_a.clone()));
    }
    document.validate().unwrap();
    let before = document.clone();
    let state = |document: &Document, port| document.graphs.main().input_state(port);
    let ghost_count = |document: &Document| document.graphs.main().ghost_wiring().bindings().len();
    let commit = |document: &mut Document, intent: GraphIntent| {
        let step = intent
            .commit_one(document, &library)
            .unwrap()
            .expect("the edit commits");
        document.validate().unwrap();
        step
    };

    let step = commit(
        &mut document,
        GraphIntent::set_input(into_b, from_a.clone()),
    );
    assert_eq!(state(&document, into_b), InputState::Live(from_a));
    assert_eq!(ghost_count(&document), 1, "the bind replaced the ghost");
    step.revert(&mut document);
    assert_eq!(document, before);

    for port in [into_b, into_ghost] {
        let step = commit(&mut document, GraphIntent::set_input(port, None));
        assert_eq!(state(&document, port), InputState::Unbound);
        assert_eq!(ghost_count(&document), 1);
        step.revert(&mut document);
        assert_eq!(document, before);
    }

    let step = commit(&mut document, GraphIntent::RemoveNode { node_id: a });
    assert_eq!(
        state(&document, into_ghost),
        InputState::Unbound,
        "the ghost wire that read `a` went with it"
    );
    assert_eq!(ghost_count(&document), 1);
    step.revert(&mut document);
    assert_eq!(document, before);

    let step = commit(&mut document, GraphIntent::RemoveNode { node_id: ghost });
    assert!(document.graphs.main().ghost_wiring().is_empty());
    step.revert(&mut document);
    assert_eq!(document, before);

    let absent = InputPort::new(NodeId::unique(), PortId::new(1));
    assert!(
        matches!(
            GraphIntent::set_input(absent, None).commit_one(&mut document, &library),
            Ok(None)
        ),
        "clearing an input of a node the graph does not hold is no edit"
    );
}

/// Select Ghosts selects each ghost node and each node a ghost touches; Remove Ghosts takes each
/// ghost node with the ghosts that touch it and clears each other ghost, as one batch whose undo
/// puts every one back.
#[test]
fn select_and_remove_ghosts_reach_every_ghost_of_the_graph() {
    let mut fixture = DocumentFixture::default();
    let [a, b, c] = [0.0, 100.0, 200.0].map(|x| fixture.node_at(Vec2::new(x, 0.0)));
    let ghost = fixture.ghost_node_at(Vec2::new(0.0, 100.0));
    let library = fixture.library;
    let mut document = fixture.document;
    let kept = |binding: Binding| InputState::Ghost(GhostBinding::from(binding));
    {
        let graph = document.graphs.main_mut();
        // The ghost node feeds `a`; `b` keeps a wire from `c`'s output 9, which `plain` lacks.
        graph.set_input_state(
            InputPort::new(a, PortId::new(1)),
            kept(Binding::wire(ghost, PortId::new(1))),
        );
        graph.set_input_state(
            InputPort::new(b, PortId::new(2)),
            kept(Binding::wire(c, PortId::new(9))),
        );
    }
    document.validate().unwrap();
    let before = document.clone();

    let GraphIntent::SetSelection { to } = GraphIntent::select_ghosts(document.graphs.main())
    else {
        panic!("Select Ghosts sets the selection");
    };
    assert_eq!(to, [a, b, c, ghost].map(ItemId::Node).into_iter().collect());

    let removals = GraphIntent::remove_ghosts(document.graphs.main());
    assert_eq!(
        removals.len(),
        2,
        "the ghost node, which takes the ghost on `a`, and the ghost on `b`: {removals:?}"
    );
    let mut steps = Vec::new();
    for intent in removals {
        intent
            .commit(&mut document, GraphId::Main, &library, &mut steps)
            .unwrap();
    }
    let graph = document.graphs.main();
    assert_eq!(graph.ghost_nodes().len(), 0);
    assert!(graph.ghost_wiring().is_empty());
    document.validate().unwrap();
    for step in steps.iter().rev() {
        step.revert(&mut document);
    }
    assert_eq!(document, before);
}

/// A wire that goes leaves its input on the declared default: cut on its own, dropped from a
/// duplicate, or gone with the node that fed it. An input with no default is left unbound, and
/// clearing a constant leaves it unbound too. Undo puts each wire back, and redo the default.
#[test]
fn a_wire_that_goes_leaves_its_input_on_its_default() {
    let mut fixture = DocumentFixture::default();
    let consumer = internals::stub_func(FuncId::unique(), "consumer")
        .input(1, InputDecl::optional("knob", DataType::Int).default(7i64))
        .input(2, InputDecl::optional("bare", DataType::Int))
        .build();
    let producer = fixture.node_at(Vec2::ZERO);
    let node = fixture.add(&consumer);
    let library = fixture.library;
    let mut document = fixture.document;
    let (knob, bare) = (
        InputPort::new(node, PortId::new(1)),
        InputPort::new(node, PortId::new(2)),
    );
    let wire = || Some(Binding::wire(producer, PortId::new(1)));
    let seven = || Some(Binding::Const(ConstValue::Int(7)));
    let wire_both = |document: &mut Document| {
        for port in [knob, bare] {
            document.graphs.main_mut().set_input_binding(port, wire());
        }
    };
    let bindings = |document: &Document| {
        [knob, bare].map(|port| document.graphs.main().bindings.get(&port).cloned())
    };

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
    let intents = GraphIntent::duplicate(
        &document,
        GraphId::Main,
        &library,
        IncomingWires::Drop,
        counter(),
    );
    let copy = commit_all(&mut document, &library, intents);
    let clone = document
        .main_view
        .selected
        .first()
        .and_then(|item| item.node())
        .unwrap();
    assert_ne!(clone, node);
    assert_eq!(
        [1, 2].map(|number| document
            .graphs
            .main()
            .bindings
            .get(&InputPort::new(clone, PortId::new(number)))
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
        matches!(
            removal.as_slice(),
            [
                UndoStep::Graph {
                    step: GraphStep::Node(_),
                    ..
                },
                UndoStep::Graph {
                    step: GraphStep::Input(_),
                    ..
                },
            ]
        ),
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
    let input = InputPort::new(dst, PortId::new(1));
    let own = Subscription::new(EventPort::new(src, PortId::new(1)), src);
    for (what, intent, expected) in [
        (
            "a text constant on an Int input",
            GraphIntent::set_input(input, Binding::Const(ConstValue::from("text"))),
            MalformedIntent::Rejected {
                port: input,
                rejection: BindingRejection::ConstMismatch,
            },
        ),
        (
            "a String wire into an Int input",
            GraphIntent::set_input(input, Binding::wire(src, PortId::new(1))),
            MalformedIntent::Rejected {
                port: input,
                rejection: BindingRejection::WireMismatch,
            },
        ),
        (
            "a node subscribing to its own event",
            GraphIntent::SetSubscription {
                subscription: own,
                to: true,
            },
            MalformedIntent::RejectedSubscription {
                subscription: own,
                rejection: SubscriptionRejection::SelfSubscription,
            },
        ),
    ] {
        assert_refused(&mut document, &library, intent, expected, what);
    }
    assert!(document.graphs.main().bindings.is_empty());
    assert_eq!(document.graphs.main().subscriptions().len(), 0);
}

/// Duplicate copies the selected graph nodes and skips a selected ghost node: a
/// copy of a node that cannot run has little use.
#[test]
fn duplicate_skips_a_selected_ghost_node() {
    let mut fixture = DocumentFixture::default();
    let node = fixture.node_at(Vec2::ZERO);
    let ghost = fixture.ghost_node_at(Vec2::new(100.0, 0.0));
    let DocumentFixture {
        mut document,
        library,
    } = fixture;
    document.main_view.selected = [node, ghost].map(ItemId::Node).into_iter().collect();

    let intents = GraphIntent::duplicate(
        &document,
        GraphId::Main,
        &library,
        IncomingWires::Drop,
        counter(),
    );
    commit_all(&mut document, &library, intents);
    assert_eq!(document.graphs.main().len(), 2, "the graph node was copied");
    assert_eq!(
        document.graphs.main().ghost_nodes().len(),
        1,
        "the ghost node was not"
    );
    document
        .validate()
        .expect("a duplicate leaves a valid document");
}

/// Inside a subgraph an edit passes over its boundary nodes: a removal of the `Outputs` node or
/// the last `Inputs` node is refused and writes nothing, a second `Inputs` node comes and goes
/// and a second `Outputs` node is refused, and a duplicate of a selection that holds both copies only the node beside
/// them. An instance of the subgraph is refused inside itself, as it would place the subgraph in
/// itself, and taken in the main graph.
#[test]
fn an_edit_inside_a_subgraph_passes_over_its_boundary_nodes() {
    let mut fixture = DocumentFixture::default();
    let id = SubgraphId::from_u128(1);
    let mut subgraph = Subgraph::new("inner", Interface::default());
    let inside = subgraph.graph.add(fixture.plain_node());
    let inputs = subgraph.inputs_node();
    let outputs = subgraph.outputs_node();
    insert_subgraph(&mut fixture.document, id, subgraph);
    let DocumentFixture {
        mut document,
        library,
    } = fixture;
    let graph = GraphId::Subgraph(id);
    let commit = |document: &mut Document, intent: GraphIntent| {
        let mut steps = Vec::new();
        intent
            .commit(document, graph, &library, &mut steps)
            .map(|()| steps)
    };

    for node_id in [inputs, outputs] {
        let before = document.clone();
        assert_eq!(
            commit(&mut document, GraphIntent::RemoveNode { node_id }).unwrap_err(),
            IntentError::Refused(EditRefusal::BoundaryNode)
        );
        assert_eq!(document, before, "{node_id:?} stays");
    }
    // A second `Inputs` node comes and goes; a second `Outputs` node never comes; the last
    // `Inputs` node stays.
    let (second, extra_outputs) = (NodeId::from_u128(40), NodeId::from_u128(41));
    commit(
        &mut document,
        GraphIntent::add_node(Vec2::ZERO, second, Node::from(Boundary::Inputs)),
    )
    .unwrap();
    assert_eq!(
        commit(
            &mut document,
            GraphIntent::add_node(Vec2::ZERO, extra_outputs, Node::from(Boundary::Outputs)),
        )
        .unwrap_err(),
        IntentError::Refused(EditRefusal::BoundaryNode)
    );
    commit(&mut document, GraphIntent::RemoveNode { node_id: inputs }).unwrap();
    assert_eq!(
        commit(&mut document, GraphIntent::RemoveNode { node_id: second }).unwrap_err(),
        IntentError::Refused(EditRefusal::BoundaryNode),
        "the last Inputs node stays"
    );
    let inputs = second;
    document.slot_mut(graph).view.selected = [inputs, outputs, inside]
        .map(ItemId::Node)
        .into_iter()
        .collect();
    let intents =
        GraphIntent::duplicate(&document, graph, &library, IncomingWires::Drop, counter());
    for intent in intents {
        commit(&mut document, intent).unwrap();
    }
    let held = document.graphs.graph(graph);
    assert_eq!(
        held.len(),
        4,
        "the two boundary nodes, the node and its copy"
    );
    assert_eq!(
        held.iter().filter(|node| node.boundary().is_some()).count(),
        2
    );

    let instance = Node::instance(id);
    let refused = GraphIntent::add_node(Vec2::ZERO, NodeId::from_u128(50), instance.clone());
    assert_eq!(
        commit(&mut document, refused).unwrap_err(),
        IntentError::Refused(EditRefusal::Placement(PlacementRejection::Recursive)),
        "no subgraph inside itself"
    );
    let taken = GraphIntent::add_node(Vec2::ZERO, NodeId::from_u128(51), instance);
    let mut steps = Vec::new();
    taken
        .commit(&mut document, GraphId::Main, &library, &mut steps)
        .unwrap();
    assert_eq!(steps.len(), 1);
    assert!(document.graphs.main().find(NodeId::from_u128(51)).is_some());
    document
        .validate()
        .expect("the edits leave a valid document");
}

/// A new node whose name is taken takes the next free number, through an add and through a
/// duplicate alike: the fixture's node, its copy, then a copy of both.
#[test]
fn a_new_node_takes_a_free_name() {
    let mut fixture = DocumentFixture::default();
    let original = fixture.node_at(Vec2::ZERO);
    let DocumentFixture {
        mut document,
        library,
    } = fixture;
    let name = document.graphs.main().find(original).unwrap().name.clone();
    let names = |document: &Document| {
        let mut names: Vec<String> = document
            .graphs
            .main()
            .iter()
            .map(|node| node.name.clone())
            .collect();
        names.sort();
        names
    };

    let node = document.graphs.main().find(original).unwrap().clone();
    commit_all(
        &mut document,
        &library,
        [GraphIntent::add_node(Vec2::ONE, NodeId::unique(), node)],
    );
    assert_eq!(names(&document), [name.clone(), format!("{name} 2")]);

    document.main_view.selected = document
        .graphs
        .main()
        .iter()
        .map(|node| ItemId::Node(node.node_id))
        .collect();
    let intents = GraphIntent::duplicate(
        &document,
        GraphId::Main,
        &library,
        IncomingWires::Drop,
        counter(),
    );
    commit_all(&mut document, &library, intents);
    assert_eq!(
        names(&document),
        [
            name.clone(),
            format!("{name} 2"),
            format!("{name} 3"),
            format!("{name} 4"),
        ]
    );
}

/// A new node's depth is fixed when its step is built, not written, so replaying history puts it
/// back where it was. Visible once a later entry restacks something: add `x` at the front, raise
/// `y` past it, then walk both back and forward. Reading `front_z()` at write time would redo the
/// add at `y`'s depth and swap the two in the paint order.
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

/// `a -> b` inside the selection, `c -> b` crossing out of it, shared by the two duplicate tests,
/// which differ only in `IncomingWires`. `b` also carries a Const on input 1, and subscribes to
/// `a`'s event, inside, and to `c`'s, crossing out.
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
    document.graphs.main_mut().set_input_binding(
        InputPort::new(b, PortId::new(1)),
        Binding::wire(a, PortId::new(1)),
    );
    document.graphs.main_mut().set_input_binding(
        InputPort::new(b, PortId::new(2)),
        Binding::Const(ConstValue::from(7i64)),
    );
    document.graphs.main_mut().set_input_binding(
        InputPort::new(b, PortId::new(3)),
        Binding::wire(c, PortId::new(1)),
    );
    document
        .graphs
        .main_mut()
        .subscribe(EventPort::new(a, PortId::new(1)), b);
    document
        .graphs
        .main_mut()
        .subscribe(EventPort::new(c, PortId::new(1)), b);
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
        .graphs
        .main()
        .iter()
        .map(|node| node.node_id)
        .find(|id| {
            !originals.contains(id)
                && document.main_view.node_placements[id].pos == origin + DUPLICATE_OFFSET
        })
        .unwrap_or_else(|| panic!("a clone offset from {origin:?}"))
}

/// Duplicating expands into ordinary intents (one add per clone, one `SetInput` per edge
/// *inside* the set, the internal subscriptions, then the selection swap), so it commits through
/// the same gate as any other edit and undoes as one batch.
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

    let intents = GraphIntent::duplicate(
        &document,
        GraphId::Main,
        &library,
        IncomingWires::Drop,
        counter(),
    );
    let steps = commit_all(&mut document, &library, intents);
    document
        .validate()
        .expect("a duplicate leaves a valid document");

    // Two fresh nodes, offset from their originals, and selected now.
    let clones: BTreeSet<NodeId> = document
        .graphs
        .main()
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

    // The internal edge and subscription are recreated against the clones, the const copies
    // verbatim, and the edge from outside the set is dropped.
    assert_eq!(
        document
            .graphs
            .main()
            .bindings
            .get(&InputPort::new(b_clone, PortId::new(1))),
        Some(&Binding::wire(a_clone, PortId::new(1))),
        "a' -> b' replaces a -> b"
    );
    assert!(matches!(
        document
            .graphs
            .main()
            .bindings
            .get(&InputPort::new(b_clone, PortId::new(2))),
        Some(Binding::Const(_))
    ));
    assert_eq!(
        document
            .graphs
            .main()
            .bindings
            .get(&InputPort::new(b_clone, PortId::new(3))),
        None,
        "the edge from unselected c is dropped"
    );
    assert!(
        document
            .graphs
            .main()
            .is_subscribed(EventPort::new(a_clone, PortId::new(1)), b_clone)
    );
    assert!(
        !document
            .graphs
            .main()
            .is_subscribed(EventPort::new(c, PortId::new(1)), b_clone),
        "the subscription to unselected c is dropped"
    );

    // One batch, so one undo.
    undo_all(&mut document, &steps);
    assert_eq!(
        document.graphs.main().len(),
        3,
        "only the originals are left"
    );
    assert_eq!(
        document.main_view.selected,
        [a, b].map(ItemId::Node).into_iter().collect()
    );
    assert_eq!(
        document
            .graphs
            .main()
            .bindings
            .get(&InputPort::new(b, PortId::new(3))),
        Some(&Binding::wire(c, PortId::new(1))),
        "the originals' wiring was never touched"
    );
    document
        .validate()
        .expect("an undone duplicate leaves a valid document");

    // Redo attaches each clone, then the edges between them.
    for step in &steps {
        step.apply(&mut document);
    }
    assert_eq!(document.graphs.main().len(), 5);
    assert_eq!(
        document.main_view.selected,
        clones.iter().copied().map(ItemId::Node).collect()
    );
    assert_eq!(
        document
            .graphs
            .main()
            .bindings
            .get(&InputPort::new(b_clone, PortId::new(1))),
        Some(&Binding::wire(a_clone, PortId::new(1)))
    );
    document
        .validate()
        .expect("a redone duplicate leaves a valid document");
}

/// The same selection with `IncomingWires::Keep`: the clone keeps the wire from the producer
/// outside the set and the subscription to the emitter outside it, each on the original, where
/// the case above drops both.
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

    let intents = GraphIntent::duplicate(
        &document,
        GraphId::Main,
        &library,
        IncomingWires::Keep,
        counter(),
    );
    commit_all(&mut document, &library, intents);
    let b_clone = clone_of(&document, Vec2::new(100.0, 0.0), &originals);
    assert_eq!(
        document
            .graphs
            .main()
            .bindings
            .get(&InputPort::new(b_clone, PortId::new(3))),
        Some(&Binding::wire(c, PortId::new(1))),
        "the kept wire still reads the original external producer"
    );
    assert!(
        document
            .graphs
            .main()
            .is_subscribed(EventPort::new(c, PortId::new(1)), b_clone),
        "the kept subscription still hears the original external emitter"
    );
    document.validate().unwrap();
}

#[test]
fn duplicate_of_nothing_asks_for_nothing() {
    let mut fixture = DocumentFixture::default();
    fixture.node_at(Vec2::ZERO);
    assert!(
        GraphIntent::duplicate(
            &fixture.document,
            GraphId::Main,
            &fixture.library,
            IncomingWires::Drop,
            counter()
        )
        .is_empty()
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
            matches!(
                GraphIntent::SetViewport { to }.commit_one(&mut document, &fixture.library),
                Err(MalformedIntent::InvalidViewport)
            ),
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
        GraphIntent::SetViewport { to: valid }
            .commit_one(&mut document, &fixture.library)
            .unwrap()
            .is_none(),
        "a finite positive viewport commits, recording no step"
    );
    assert_eq!(document.main_view.viewport, valid);
    assert_eq!(document.main_view.viewport, valid);
    document.validate().unwrap();
}

/// Each one-step edit replays exactly: its revert gives back the whole document as before the
/// commit, and its apply the whole document after it, not just the field it is about.
#[test]
fn each_edit_reverts_and_redoes_the_whole_document() {
    let mut fixture = DocumentFixture::default();
    let a = fixture.node_at(Vec2::ZERO);
    let b = fixture.node_at(Vec2::new(100.0, 0.0));
    let mut document = fixture.document;
    let edits = [
        (
            "rename",
            GraphIntent::RenameNode {
                node_id: a,
                to: "renamed".to_string(),
            },
        ),
        (
            "disable",
            GraphIntent::SetNodeProperty {
                node_id: a,
                to: NodeProperty::Disabled(true),
            },
        ),
        (
            "cache to disk",
            GraphIntent::SetNodeProperty {
                node_id: b,
                to: NodeProperty::RuntimeCache(CacheMode::Disk),
            },
        ),
        ("subscribe", subscribe(a, b, true)),
        ("unsubscribe", subscribe(a, b, false)),
        (
            "select",
            GraphIntent::SetSelection {
                to: BTreeSet::from([ItemId::Node(a), ItemId::Node(b)]),
            },
        ),
        (
            "deselect one",
            GraphIntent::SetSelection {
                to: BTreeSet::from([ItemId::Node(b)]),
            },
        ),
        ("raise", GraphIntent::Raise { node_id: a }),
        (
            "bind",
            GraphIntent::set_input(
                InputPort::new(b, PortId::new(1)),
                Binding::wire(a, PortId::new(1)),
            ),
        ),
    ];
    for (what, intent) in edits {
        let before = document.clone();
        let step = intent
            .commit_one(&mut document, &fixture.library)
            .unwrap()
            .unwrap_or_else(|| panic!("{what} commits a step"));
        let after = document.clone();
        assert_ne!(after, before, "{what} changed something");
        step.revert(&mut document);
        assert_eq!(document, before, "{what}: the revert");
        step.apply(&mut document);
        assert_eq!(document, after, "{what}: the redo");
    }
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
            .graphs
            .main()
            .is_subscribed(EventPort::new(emitter, PortId::new(1)), subscriber)
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
            .graphs
            .main()
            .is_subscribed(EventPort::new(emitter, PortId::new(1)), subscriber)
    );
    step.apply(&mut document);
    assert!(
        document
            .graphs
            .main()
            .is_subscribed(EventPort::new(emitter, PortId::new(1)), subscriber)
    );

    // Unsubscribe commits, removes the edge, and undo brings it back.
    let step = subscribe(emitter, subscriber, false)
        .commit_one(&mut document, &fixture.library)
        .unwrap()
        .expect("unsubscribe commits");
    assert!(
        !document
            .graphs
            .main()
            .is_subscribed(EventPort::new(emitter, PortId::new(1)), subscriber)
    );
    step.revert(&mut document);
    assert!(
        document
            .graphs
            .main()
            .is_subscribed(EventPort::new(emitter, PortId::new(1)), subscriber)
    );

    // Redo the unsubscribe (apply writes the `to = unsubscribed` half),
    // then unsubscribing the now-absent edge is a no-op.
    step.apply(&mut document);
    assert!(
        !document
            .graphs
            .main()
            .is_subscribed(EventPort::new(emitter, PortId::new(1)), subscriber)
    );
    assert!(
        subscribe(emitter, subscriber, false)
            .commit_one(&mut document, &fixture.library)
            .unwrap()
            .is_none(),
        "unsubscribing a missing edge is a no-op"
    );
}

/// An instance that would nest the main graph past [`ProgramShape::MAX_LEVELS`] is refused,
/// naming the depth it would reach, and adds nothing: subgraph `k` places subgraph `k + 1` and
/// the last holds one node, so the first is `MAX_LEVELS` deep, and an instance of it one level
/// more. An instance of the second fits.
#[test]
fn an_add_past_a_placement_bound_is_refused() {
    let mut fixture = DocumentFixture::default();
    let body = fixture.plain_node();
    let ids: Vec<SubgraphId> = (1..=ProgramShape::MAX_LEVELS as u128)
        .map(SubgraphId::from_u128)
        .collect();
    let empty = || Subgraph::new("level", Interface::default());
    let mut document = fixture.document;
    let mut inner = empty();
    inner.graph.add(body);
    for pair in ids.windows(2).rev() {
        let mut outer = empty();
        outer.graph.add(Node::instance(pair[1]));
        insert_subgraph(&mut document, pair[1], inner);
        inner = outer;
    }
    let deepest_first = inner;
    let too_deep = Node::instance(ids[0]);
    let fits = Node::instance(ids[1]);
    insert_subgraph(&mut document, ids[0], deepest_first);

    let refused = NodeId::unique();
    let mut steps = Vec::new();
    assert_eq!(
        GraphIntent::add_node(Vec2::ZERO, refused, too_deep)
            .commit(&mut document, GraphId::Main, &fixture.library, &mut steps)
            .unwrap_err(),
        IntentError::Refused(EditRefusal::Placement(PlacementRejection::TooDeep {
            graph: GraphId::Main,
            levels: ProgramShape::MAX_LEVELS + 1,
        }))
    );
    assert!(steps.is_empty());
    assert!(!document.graphs.main().holds(refused));
    let added = NodeId::unique();
    assert!(
        GraphIntent::add_node(Vec2::ZERO, added, fits)
            .commit_one(&mut document, &fixture.library)
            .unwrap()
            .is_some()
    );
    assert!(document.graphs.main().holds(added));
}

#[test]
fn set_node_property_commits_and_reverts() {
    let mut fixture = DocumentFixture::default();
    let id = fixture.node_at(Vec2::ZERO);
    let mut document = fixture.document;
    // Fresh nodes default to no caching (None) and enabled.
    assert_eq!(
        document.graphs.main().find(id).unwrap().cache(),
        Some(CacheMode::None)
    );
    assert!(!document.graphs.main().find(id).unwrap().disabled);

    // A representative flip of each property, committed then reverted, so each iteration returns
    // the node to its defaults and the step's captured `from` is always None / enabled.
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
        let node = document.graphs.main().find(id).unwrap();
        match to {
            NodeProperty::RuntimeCache(m) => assert_eq!(node.cache(), Some(m)),
            NodeProperty::Disabled(d) => assert_eq!(node.disabled, d),
            NodeProperty::Subgraph(_) => unreachable!("no case re-points"),
        }
        assert_eq!(
            step.facts(),
            main_touched(),
            "a node-property toggle changes no shape"
        );
        step.revert(&mut document);
        let node = document.graphs.main().find(id).unwrap();
        assert_eq!(
            node.cache(),
            Some(CacheMode::None),
            "revert restores the cache"
        );
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

/// A node with no body, an instance or a boundary node, caches nothing of its own, so a cache
/// mode set on one is malformed, while its disable flag is an ordinary edit.
#[test]
fn a_cache_mode_on_a_node_with_no_body_is_malformed() {
    let mut fixture = DocumentFixture::default();
    let node_id = NodeId::unique();
    fixture
        .document
        .graphs
        .main_mut()
        .insert(node_id, Node::from(Boundary::Inputs));
    let mut document = fixture.document;
    assert_eq!(
        GraphIntent::SetNodeProperty {
            node_id,
            to: NodeProperty::RuntimeCache(CacheMode::Ram),
        }
        .commit_one(&mut document, &fixture.library)
        .unwrap_err(),
        MalformedIntent::NoCacheMode { node_id }
    );
    assert!(
        GraphIntent::SetNodeProperty {
            node_id,
            to: NodeProperty::Disabled(true),
        }
        .commit_one(&mut document, &fixture.library)
        .unwrap()
        .is_some()
    );
}

#[test]
fn commit_rejects_cycle_forming_bind() {
    // a → b (b's input 0 bound to a's output 0).
    let mut fixture = DocumentFixture::default();
    let a = fixture.node_at(Vec2::ZERO);
    let b = fixture.node_at(Vec2::new(100.0, 0.0));
    let c = fixture.node_at(Vec2::new(0.0, 100.0));
    let mut document = fixture.document;
    document.graphs.main_mut().set_input_binding(
        InputPort::new(b, PortId::new(1)),
        Binding::wire(a, PortId::new(1)),
    );

    // Wiring a's input back to b's output would close the a → b loop:
    // rejected, nothing written, the existing edge untouched.
    assert!(
        GraphIntent::SetInput {
            input: InputPort::new(a, PortId::new(1)),
            to: Some(Binding::wire(b, PortId::new(1))),
        }
        .commit_one(&mut document, &fixture.library)
        .unwrap()
        .is_none(),
        "a bind that closes a cycle is rejected"
    );
    assert_eq!(
        document
            .graphs
            .main()
            .bindings
            .get(&InputPort::new(a, PortId::new(1))),
        None,
        "the rejected bind left a's input unbound"
    );
    assert_eq!(
        document
            .graphs
            .main()
            .bindings
            .get(&InputPort::new(b, PortId::new(1))),
        Some(&Binding::wire(a, PortId::new(1))),
        "the existing a → b edge is untouched"
    );

    // A bind that keeps the graph acyclic still commits: c's input ← b's
    // output extends the chain into a → b → c.
    assert!(
        GraphIntent::SetInput {
            input: InputPort::new(c, PortId::new(1)),
            to: Some(Binding::wire(b, PortId::new(1))),
        }
        .commit_one(&mut document, &fixture.library)
        .unwrap()
        .is_some(),
        "an acyclic bind commits"
    );
    assert_eq!(
        document
            .graphs
            .main()
            .bindings
            .get(&InputPort::new(c, PortId::new(1))),
        Some(&Binding::wire(b, PortId::new(1))),
    );
}

/// Commit `intent`, which the gate refuses as `expected`, and check the refusal left `document`
/// exactly as it was.
#[track_caller]
fn assert_refused(
    document: &mut Document,
    library: &Library,
    intent: GraphIntent,
    expected: MalformedIntent,
    what: &str,
) {
    let before = document.clone();
    assert_eq!(
        intent.commit_one(document, library).map(|_| ()),
        Err(expected),
        "{what}"
    );
    assert_eq!(*document, before, "{what}: a refusal writes nothing");
}

#[test]
fn insertions_reusing_an_identity_are_refused_instead_of_panicking() {
    // A live id would otherwise abort inside the write half on `Graph::insert`'s duplicate-id
    // panic, and a nil one names nothing a widget could have read.
    let mut fixture = DocumentFixture::default();
    let live = fixture.node_at(Vec2::ZERO);
    let ghost = fixture.ghost_node_at(Vec2::new(100.0, 0.0));
    let node = fixture.plain_node();
    let mut document = fixture.document;

    let library = &fixture.library;
    for (what, intent, expected) in [
        (
            "AddNode over a ghost node's id",
            GraphIntent::add_node(Vec2::ZERO, ghost, node.clone()),
            MalformedIntent::NodeAlreadyExists { node_id: ghost },
        ),
        (
            "AddNode over a live id",
            GraphIntent::add_node(Vec2::ZERO, live, node.clone()),
            MalformedIntent::NodeAlreadyExists { node_id: live },
        ),
        (
            "AddNode with a nil id",
            GraphIntent::add_node(Vec2::ZERO, NodeId::nil(), DocumentFixture::missing_node()),
            MalformedIntent::NilNodeId { role: Role::New },
        ),
    ] {
        assert_refused(&mut document, library, intent, expected, what);
    }

    // Intents commit one at a time, so an id repeated *within* one batch is
    // refused by the same check: the second add meets a graph the first is
    // already in.
    let repeated = NodeId::unique();
    assert!(
        GraphIntent::add_node(Vec2::ZERO, repeated, node)
            .commit_one(&mut document, library)
            .unwrap()
            .is_some()
    );
    assert_refused(
        &mut document,
        library,
        GraphIntent::add_node(Vec2::ONE, repeated, DocumentFixture::missing_node()),
        MalformedIntent::NodeAlreadyExists { node_id: repeated },
        "a second AddNode repeating an id from the same batch",
    );
}

#[test]
fn malformed_payloads_are_refused_before_they_can_invalidate_the_document() {
    // Every case here would apply cleanly and leave a document `Document::validate` rejects, or
    // trip an assert inside the graph; saving only validates in debug builds, so it would write
    // fine and not reopen.
    let probe = internals::stub_func(FuncId::unique(), "probe")
        .input(1, InputDecl::optional("a", DataType::Int))
        .output(1, OutputDecl::new("out", DataType::Int))
        .build();
    let mut fixture = DocumentFixture::default();
    let live = fixture.add(&probe);
    let mut document = fixture.document;
    let ghost = NodeId::unique();
    // Infinite rather than NaN, which no error equals.
    let far = Vec2::new(f32::INFINITY, 0.0);

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
    let own = |number| InputPort::new(fresh, PortId::new(number));
    let cases = [
        (
            "AddNode at a non-finite position",
            GraphIntent::add_node(far, NodeId::unique(), DocumentFixture::missing_node()),
            MalformedIntent::NonFinitePos {
                role: Role::AddNode,
                pos: far,
            },
        ),
        (
            "AddNode with a nil func id",
            GraphIntent::add_node(Vec2::ZERO, NodeId::unique(), nil_func),
            MalformedIntent::UnresolvedNode {
                reason: Unresolved::MissingFunc,
            },
        ),
        (
            // The graph holds only nodes the library can run.
            "AddNode of a node whose func the library lacks",
            GraphIntent::add_node(
                Vec2::ZERO,
                NodeId::unique(),
                DocumentFixture::missing_node(),
            ),
            MalformedIntent::UnresolvedNode {
                reason: Unresolved::MissingFunc,
            },
        ),
        (
            "AddNode seeding a binding from a producer that isn't there",
            seeded(vec![(own(1), Binding::wire(ghost, PortId::new(1)))]),
            MalformedIntent::NodeAbsent {
                role: Role::BindingProducer,
                node_id: ghost,
            },
        ),
        (
            // The insertion restores exactly the wiring it recorded, so it may
            // only author its own node's inputs.
            "AddNode seeding a binding on someone else's port",
            seeded(vec![(
                InputPort::new(live, PortId::new(1)),
                Binding::wire(fresh, PortId::new(1)),
            )]),
            MalformedIntent::ForeignSeedBinding {
                port: InputPort::new(live, PortId::new(1)),
            },
        ),
        (
            // A loop of one: nothing reads the new node yet, so its own output
            // is the only source that could close a cycle — and a compile
            // refuses a cyclic graph.
            "AddNode seeding an input from the new node's own output",
            seeded(vec![(own(1), Binding::wire(fresh, PortId::new(1)))]),
            MalformedIntent::CyclicSeedBinding { port: own(1) },
        ),
        (
            // Only one could survive, so `DetachedNode::new` refuses the record.
            "AddNode seeding one port twice",
            seeded(vec![
                (own(1), Binding::Const(ConstValue::from(1i64))),
                (own(1), Binding::Const(ConstValue::from(2i64))),
            ]),
            MalformedIntent::InsertionRecord(DetachedNodeError::DuplicateBinding { port: own(1) }),
        ),
        (
            "MoveSelection to a non-finite position",
            drag(&[member(live, far)], Vec2::ZERO),
            MalformedIntent::NonFinitePos {
                role: Role::MoveSelection,
                pos: far,
            },
        ),
        (
            "SetSubscription from a nil emitter",
            subscribe(NodeId::nil(), live, true),
            MalformedIntent::NilNodeId {
                role: Role::SetSubscriptionEmitter,
            },
        ),
        (
            "SetSubscription to a nil subscriber",
            subscribe(live, NodeId::nil(), true),
            MalformedIntent::NilNodeId {
                role: Role::SetSubscriptionSubscriber,
            },
        ),
    ];
    for (what, intent, expected) in cases {
        assert_refused(&mut document, &fixture.library, intent, expected, what);
    }

    assert_eq!(
        Role::SetInputProducer.to_string(),
        "SetInput producer",
        "a role reads as the intent and field it names"
    );
}

#[test]
fn stale_references_still_refuse_quietly() {
    // Widgets read identities out of the live document, so they only get staleness wrong: an
    // anchor removed between the gesture starting and the intent draining. Those stay silent, as
    // reporting them would spam the status bar on ordinary use.
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
                input: InputPort::new(gone, PortId::new(1)),
                to: None,
            },
        ),
        (
            // The held-wire case: the producer was removed after the drag
            // began, so committing would leave a dangling edge.
            "SetInput from a vanished producer",
            GraphIntent::SetInput {
                input: InputPort::new(live, PortId::new(1)),
                to: Some(Binding::wire(gone, PortId::new(1))),
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
    assert!(
        document.graphs.main().bindings.is_empty(),
        "nothing was written"
    );
    document.validate().expect("document stays valid");
}

#[test]
fn selection_and_move_drop_members_whose_widget_is_gone() {
    // The rubber band and a group drag snapshot identities at gesture start, and undo runs before
    // the gesture input stage, so a member can disappear mid-gesture. Recording it verbatim would
    // leave `selected` naming an absent item: `GraphViewValidationError::MissingSelectedItem`.
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
    let UndoStep::Graph {
        step: GraphStep::Selection(step),
        ..
    } = &step
    else {
        panic!("expected a SetSelection step, got {step:?}");
    };
    let kept: BTreeSet<ItemId> = [ItemId::Node(live)].into_iter().collect();
    assert_eq!(
        step.toggled, kept,
        "the vanished node and the tie no view holds are dropped, the live one kept"
    );
    assert_eq!(document.main_view.selected, kept);

    // `live` latched at (1, 2) and sits at the origin now: the step moves it
    // from where it sits to (1, 2) + (4, 4) = (5, 6).
    let step = drag(
        &[member(live, Vec2::new(1.0, 2.0)), member(gone, Vec2::ZERO)],
        Vec2::new(4.0, 4.0),
    )
    .commit_one(&mut document, &fixture.library)
    .unwrap()
    .expect("a move with one live member commits");
    let UndoStep::Graph {
        step: GraphStep::Positions(step),
        ..
    } = &step
    else {
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

/// A wire onto an input its func declares const-only is refused at the gate, on an existing node
/// and as an insertion's seed alike; a constant on the same input commits.
#[test]
fn a_wire_onto_a_const_only_input_is_refused() {
    let knob_func = internals::stub_func(FuncId::unique(), "knob")
        .input(
            1,
            InputDecl::required("strength", DataType::Float).const_only(),
        )
        .build();
    let feed_func = internals::stub_func(FuncId::unique(), "feed")
        .output(1, OutputDecl::new("value", DataType::Float))
        .build();
    let mut fixture = DocumentFixture::default();
    let knob = fixture.add(&knob_func);
    let feed = fixture.add(&feed_func);
    let mut document = fixture.document;
    let input = InputPort::new(knob, PortId::new(1));

    let fresh = NodeId::unique();
    for (what, intent, port) in [
        (
            "a wire onto it",
            GraphIntent::SetInput {
                input,
                to: Some(Binding::wire(feed, PortId::new(1))),
            },
            input,
        ),
        (
            "an insertion seeding a wire onto it",
            GraphIntent::AddNode {
                pos: Vec2::ZERO,
                node_id: fresh,
                node: Node::from(&knob_func),
                bindings: vec![BindingEntry {
                    port: InputPort::new(fresh, PortId::new(1)),
                    binding: Binding::wire(feed, PortId::new(1)),
                }],
            },
            InputPort::new(fresh, PortId::new(1)),
        ),
    ] {
        let expected = MalformedIntent::Rejected {
            port,
            rejection: BindingRejection::WiredConstOnly,
        };
        assert_refused(&mut document, &fixture.library, intent, expected, what);
    }

    let constant = GraphIntent::SetInput {
        input,
        to: Some(Binding::Const(ConstValue::Float(0.5))),
    }
    .commit_one(&mut document, &fixture.library);
    assert!(matches!(constant, Ok(Some(_))), "{constant:?}");
}

/// Every port an intent writes is held to the node's declaration: the probe declares input,
/// output and event 1, so port 1 of each commits and port 2 is refused, on an existing node and
/// on an insertion's seed alike. Clearing an undeclared input is not a write.
#[test]
fn ports_the_declaration_lacks_are_refused() {
    let probe = internals::stub_func(FuncId::unique(), "probe")
        .input(1, InputDecl::optional("a", DataType::Int))
        .output(1, OutputDecl::new("out", DataType::Int))
        .event(1, "tick", internals::stub_event())
        .build();
    let mut fixture = DocumentFixture::default();
    let (producer, consumer) = (fixture.add(&probe), fixture.add(&probe));
    let mut document = fixture.document;
    let library = &fixture.library;
    let constant = || Some(Binding::Const(ConstValue::from(1i64)));
    let set = |number: u32, to: Option<Binding>| GraphIntent::SetInput {
        input: InputPort::new(consumer, PortId::new(number)),
        to,
    };
    let fresh = NodeId::unique();
    let seeded = |number: u32, binding: Binding| GraphIntent::AddNode {
        pos: Vec2::ZERO,
        node_id: fresh,
        node: Node::from(&probe),
        bindings: vec![BindingEntry {
            port: InputPort::new(fresh, PortId::new(number)),
            binding,
        }],
    };
    let listen = |number: u32| GraphIntent::SetSubscription {
        subscription: Subscription::new(EventPort::new(producer, PortId::new(number)), consumer),
        to: true,
    };

    let rejected = |port, rejection| MalformedIntent::Rejected { port, rejection };
    let refusals = [
        (
            "a constant on input 2",
            set(2, constant()),
            rejected(
                InputPort::new(consumer, PortId::new(2)),
                BindingRejection::UndeclaredInput,
            ),
        ),
        (
            "a wire from output 2",
            set(1, Some(Binding::wire(producer, PortId::new(2)))),
            rejected(
                InputPort::new(consumer, PortId::new(1)),
                BindingRejection::UndeclaredOutput,
            ),
        ),
        (
            "a seed constant on input 2",
            seeded(2, Binding::Const(ConstValue::from(1i64))),
            rejected(
                InputPort::new(fresh, PortId::new(2)),
                BindingRejection::UndeclaredInput,
            ),
        ),
        (
            "a seed wire from output 2",
            seeded(1, Binding::wire(producer, PortId::new(2))),
            rejected(
                InputPort::new(fresh, PortId::new(1)),
                BindingRejection::UndeclaredOutput,
            ),
        ),
        (
            "a subscription to event 2",
            listen(2),
            MalformedIntent::RejectedSubscription {
                subscription: Subscription::new(EventPort::new(producer, PortId::new(2)), consumer),
                rejection: SubscriptionRejection::UndeclaredEvent,
            },
        ),
    ];
    for (what, intent, expected) in refusals {
        assert_refused(&mut document, library, intent, expected, what);
    }

    assert!(
        matches!(set(2, None).commit_one(&mut document, library), Ok(None)),
        "clearing an undeclared input writes nothing and refuses nothing"
    );
    for (what, intent) in [
        ("a constant on input 1", set(1, constant())),
        (
            "a wire from output 1",
            set(1, Some(Binding::wire(producer, PortId::new(1)))),
        ),
        ("a subscription to event 1", listen(1)),
        (
            "a seed wire from output 1",
            seeded(1, Binding::wire(producer, PortId::new(1))),
        ),
    ] {
        let committed = intent.commit_one(&mut document, library);
        assert!(matches!(committed, Ok(Some(_))), "{what}: {committed:?}");
    }
}

/// What an edit case names, the edit, and what it checks of the document
/// after it.
type Edit<'a> = (&'a str, GraphIntent, &'a dyn Fn(&Document));

/// Three plain nodes in a row and four wires, each end its port's number: `ab =
/// a.1 → b.1`, `ac = a.1 → c.1`, `bc = b.1 → c.2` and the event wire `tick = a.tick → b`. The tie
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
    let ab = fixture.wire(
        OutputPort::new(a, PortId::new(1)),
        InputPort::new(b, PortId::new(1)),
    );
    let ac = fixture.wire(
        OutputPort::new(a, PortId::new(1)),
        InputPort::new(c, PortId::new(1)),
    );
    let bc = fixture.wire(
        OutputPort::new(b, PortId::new(1)),
        InputPort::new(c, PortId::new(2)),
    );
    let tick = fixture.subscribe(EventPort::new(a, PortId::new(1)), b);
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

/// Each tie edit lands as one `SetTies` step, a tie's move as a `SetPositions`; a revert puts the
/// document back exactly. A tie edit dirties the document and leaves the node set and the
/// geometry caches alone.
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
        if let UndoStep::Graph {
            step: GraphStep::Ties(_),
            ..
        } = step
        {
            assert!(step.dirties_document(), "{what}");
            assert_eq!(step.facts(), main_touched(), "{what}");
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

/// A wire is both of its ends: rewiring `c.0` from `a` to `b` drops the route of `ac`, while
/// `shared` stays for `bc`; cutting `b.0` drops the route of `ab`, while `held` stays for `tick`;
/// unsubscribing `tick` drops its route. A bind to the same output records no step and keeps the
/// route. Removing `a` ends `ab`, `ac` and `tick`: `held`, which only they held, goes, and
/// `shared` stays for `bc`. Each revert brings all of it back.
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
    let input = InputPort::new(b, PortId::new(1));
    let mut same = document.clone();
    assert!(
        GraphIntent::set_input(input, Binding::wire(a, PortId::new(1)))
            .commit_one(&mut same, &library)
            .unwrap()
            .is_none(),
        "a bind to the same output is no edit"
    );
    assert_eq!(same, document, "and keeps the route");

    let cases: [Edit<'_>; 4] = [
        (
            "a rewire to another output",
            GraphIntent::set_input(
                InputPort::new(c, PortId::new(1)),
                Binding::wire(b, PortId::new(1)),
            ),
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
    let unheld = WireKey::Event(Subscription::new(EventPort::new(c, PortId::new(1)), a));
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
            // Infinite rather than NaN, which no error equals.
            "a position that is not finite",
            GraphIntent::TieWires {
                tie: fresh,
                pos: Vec2::INFINITY,
                wires: vec![RouteInsert { wire: ab, at: 0 }],
            },
            MalformedIntent::NonFinitePos {
                role: Role::TieWires,
                pos: Vec2::INFINITY,
            },
        ),
        // A nil id is malformed, not stale: no view holds one, and it is
        // refused before the lookup could read it as merely absent.
        (
            "a new tie with a nil id",
            tie_wires(TieId::nil(), vec![RouteInsert { wire: ab, at: 0 }]),
            MalformedIntent::NilTieId {
                role: Role::TieWires,
            },
        ),
        (
            "a join of a nil tie",
            GraphIntent::JoinTie {
                tie: TieId::nil(),
                wires: vec![RouteInsert { wire: ab, at: 0 }],
            },
            MalformedIntent::NilTieId {
                role: Role::JoinTie,
            },
        ),
        (
            "a removal of a nil tie",
            GraphIntent::RemoveTie { tie: TieId::nil() },
            MalformedIntent::NilTieId {
                role: Role::RemoveTie,
            },
        ),
        (
            "an angle of a nil tie",
            GraphIntent::SetTieAngle {
                tie: TieId::nil(),
                angle: TieAngle::Auto,
                gesture: None,
            },
            MalformedIntent::NilTieId {
                role: Role::SetTieAngle,
            },
        ),
        (
            "a release from a nil tie",
            GraphIntent::ReleaseWire {
                tie: TieId::nil(),
                wire: ab,
            },
            MalformedIntent::NilTieId {
                role: Role::ReleaseWire,
            },
        ),
        (
            "a raise of a nil node",
            GraphIntent::Raise {
                node_id: NodeId::nil(),
            },
            MalformedIntent::NilNodeId { role: Role::Raise },
        ),
    ];
    let before = document.clone();
    for (what, intent, expected) in refused {
        assert_refused(&mut document, &library, intent, expected, what);
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

/// A duplicate of `a` and `b` takes the ties inside them, at the offset and with their angles,
/// and the cloned wire runs through the clones in its original's order: `ab` runs through
/// `second` then `first`, so `ab'` runs through `second'` then `first'`. `mixed` also holds `cb`,
/// from `c` outside the set, so even with `IncomingWires::Keep`, which keeps `cb'`, it is not
/// cloned, and `cb'` has no route. The clones end up selected with the copied nodes, and one undo
/// takes all of it back.
#[test]
fn duplicate_takes_the_ties_inside_the_set() {
    let mut fixture = DocumentFixture::default();
    let [a, b, c] = [0.0, 220.0, 440.0].map(|x| fixture.node_at(Vec2::new(x, 0.0)));
    let ab = fixture.wire(
        OutputPort::new(a, PortId::new(1)),
        InputPort::new(b, PortId::new(1)),
    );
    let cb = fixture.wire(
        OutputPort::new(c, PortId::new(1)),
        InputPort::new(b, PortId::new(2)),
    );
    let first = fixture.tie(Vec2::new(150.0, 0.0), [ab]);
    let second = fixture.tie(Vec2::new(60.0, 0.0), [ab]);
    let mixed = fixture.tie(Vec2::new(300.0, 0.0), [cb]);
    let DocumentFixture {
        mut document,
        library,
    } = fixture;
    // An event wire `a.tick → b` through `first` too, which clones as a data wire does.
    let tick = Subscription::new(EventPort::new(a, PortId::new(1)), b);
    document.graphs.main_mut().subscribe(tick.event, b);
    let view = &mut document.main_view;
    view.routes.insert(ab, Route::new([second, first, mixed]));
    view.routes
        .insert(WireKey::Event(tick), Route::new([first]));
    view.ties.get_mut(&first).unwrap().angle = TieAngle::Pinned(30.0);
    view.selected = [ItemId::Node(a), ItemId::Node(b)].into_iter().collect();
    document.validate().unwrap();
    let before = document.clone();

    let intents = GraphIntent::duplicate(
        &document,
        GraphId::Main,
        &library,
        IncomingWires::Keep,
        counter(),
    );
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
        output: OutputPort::new(a_clone, PortId::new(1)),
        input: InputPort::new(b_clone, PortId::new(1)),
    });
    let cb_clone = WireKey::Data(DataWire {
        output: OutputPort::new(c, PortId::new(1)),
        input: InputPort::new(b_clone, PortId::new(2)),
    });
    assert!(
        cb_clone.is_held_by(document.graphs.main()),
        "the kept incoming wire"
    );
    assert_eq!(
        view.routes[&ab_clone],
        Route::new([second_clone, first_clone])
    );
    assert!(!view.routes.contains_key(&cb_clone));
    let tick_clone = WireKey::Event(Subscription::new(
        EventPort::new(a_clone, PortId::new(1)),
        b_clone,
    ));
    assert_eq!(view.routes[&tick_clone], Route::new([first_clone]));
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
