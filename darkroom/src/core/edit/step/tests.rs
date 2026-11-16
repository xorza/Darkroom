//! The per-step predicates the undo history and the frame pipeline read off a
//! step.
//!
//! The frame-cost questions are swept over one representative of every kind,
//! and a held gesture's fold is checked on the kinds that fold.

use std::collections::BTreeSet;

use glam::Vec2;
use scenarium::{
    Binding, CacheMode, ConstValue, DataType, EventPort, GhostBinding, GraphId, InputDecl,
    InputPort, InputState, Interface, NodeId, Numbered, OutputPort, PortId, Ports, RefusedFate,
    Subgraph, SubgraphId, Subscription, SubscriptionState,
};

use crate::core::document::graph_view::GraphView;
use crate::core::document::internals::{DocumentFixture, insert_subgraph};
use crate::core::document::item_id::ItemId;
use crate::core::document::subgraph_side::SideMarks;
use crate::core::document::subgraph_side::SubgraphSide;
use crate::core::edit::edit_facts::{EditFacts, Touched};
use crate::core::edit::graph_intent::GraphIntent;
use crate::core::edit::step::change::Change;
use crate::core::edit::step::graph_step::GraphStep;
use crate::core::edit::step::set_definition::SetDefinition;
use crate::core::edit::step::set_depth::SetDepth;
use crate::core::edit::step::set_input::SetInput;
use crate::core::edit::step::set_interface::SetInterface;
use crate::core::edit::step::set_node::{NodeEntry, SetNode};
use crate::core::edit::step::set_node_name::SetNodeName;
use crate::core::edit::step::set_node_property::{PropertyChange, SetNodeProperty};
use crate::core::edit::step::set_positions::{Move, SetPositions};
use crate::core::edit::step::set_selection::SetSelection;
use crate::core::edit::step::set_side::SetSide;
use crate::core::edit::step::set_subgraph::SetSubgraph;
use crate::core::edit::step::set_subgraph_name::SetSubgraphName;
use crate::core::edit::step::set_subscription::SetSubscription;
use crate::core::edit::step::set_ties::SetTies;
use crate::core::edit::step::undo_step::UndoStep;

/// `step` on the main graph.
const fn main(step: GraphStep) -> UndoStep {
    UndoStep::Graph {
        graph: GraphId::Main,
        step,
    }
}

fn set_positions(node_id: NodeId, from: Vec2, to: Vec2) -> UndoStep {
    main(GraphStep::Positions(SetPositions {
        moves: vec![Move {
            item: ItemId::Node(node_id),
            pos: Change { from, to },
        }],
    }))
}

fn set_input(input: InputPort, from: Option<Binding>, to: Option<Binding>) -> UndoStep {
    main(GraphStep::Input(Box::new(SetInput {
        input,
        state: Change {
            from: from.into(),
            to: to.into(),
        },
        ties: SetTies::default(),
    })))
}

/// An interface edit that removes the subgraph's one input, on which an instance in the main
/// graph holds a constant: the edit drops it.
fn interface_port_removal() -> UndoStep {
    let port = PortId::new(1);
    let interface = Interface::new(Ports {
        inputs: vec![Numbered::new(port, InputDecl::optional("a", DataType::Int))],
        outputs: vec![],
    });
    let id = SubgraphId::from_u128(9);
    let mut fixture = DocumentFixture::default();
    insert_subgraph(&mut fixture.document, id, Subgraph::new("inner", interface));
    let instance = fixture.add_instance(id);
    fixture.document.graphs.main_mut().bindings.insert(
        InputPort::new(instance, port),
        Binding::Const(ConstValue::Int(3)),
    );
    UndoStep::Interface(Box::new(SetInterface::plan(
        &fixture.document,
        id,
        Interface::default(),
        &fixture.library,
        RefusedFate::Drop,
    )))
}

fn constant(v: f64) -> Binding {
    Binding::Const(ConstValue::Float(v))
}

fn renamed(node_id: NodeId, from: &str, to: &str) -> UndoStep {
    main(GraphStep::NodeName(SetNodeName {
        node_id,
        name: Change {
            from: from.into(),
            to: to.into(),
        },
    }))
}

/// A removal of a ghost node — the one kind that can only be built by reading a
/// document, since it carries everything the document held about the node.
fn node_removal() -> UndoStep {
    let mut fixture = DocumentFixture::default();
    let node_id = fixture.ghost_node_at(Vec2::ZERO);
    let entry = NodeEntry::capture(&fixture.document, GraphId::Main, node_id)
        .expect("the fixture placed it");
    main(GraphStep::Node(Box::new(SetNode::removal(
        entry,
        SetTies::default(),
    ))))
}

/// A removal of a tie, read off a document that holds one over a wire.
fn tie_removal() -> UndoStep {
    let mut fixture = DocumentFixture::default();
    let [a, b] = [0.0, 200.0].map(|x| fixture.node_at(Vec2::new(x, 0.0)));
    let wire = fixture.wire(
        OutputPort::new(a, PortId::new(1)),
        InputPort::new(b, PortId::new(1)),
    );
    let tie = fixture.tie(Vec2::new(100.0, 0.0), [wire]);
    GraphIntent::RemoveTie { tie }
        .commit_one(&mut fixture.document, &fixture.library)
        .unwrap()
        .expect("a held tie's removal is a step")
}

/// The frame-cost questions, over one representative of *every* kind — a
/// kind whose answer is wrong shows up here rather than as a save prompt that
/// never fires, a drag that pays for two record passes, or a removed node's
/// cache entries left behind:
///
/// - **dirties**: navigation (selection, stacking) leaves the unsaved flag
///   alone; graph data and layout flip it.
/// - **facts**: only a node coming or going changes the node set, so a drag frame
///   sweeps no node cache; only a rename renames; and only an input that turns between a const
///   and a wire or nothing changes its binding kind, which a value edit does not.
///
/// The cases are matched against every variant, so a new kind does not
/// compile until it has one here.
#[test]
fn every_kind_answers_the_frame_questions() {
    let node_id = NodeId::unique();
    let port = InputPort::new(node_id, PortId::new(1));
    // (what, step, [dirties, changes node set, invalidates geometry])
    let cases = [
        (
            "node removal",
            node_removal(),
            (
                true,
                EditFacts {
                    node_set: true,
                    ..EditFacts::NONE
                },
            ),
        ),
        (
            "move",
            set_positions(node_id, Vec2::ZERO, Vec2::new(5.0, 5.0)),
            (true, EditFacts::NONE),
        ),
        (
            "rename",
            renamed(node_id, "a", "a-much-longer-title"),
            (
                true,
                EditFacts {
                    renamed: true,
                    ..EditFacts::NONE
                },
            ),
        ),
        (
            "value edit",
            set_input(port, Some(constant(1.0)), Some(constant(2.0))),
            (true, EditFacts::NONE),
        ),
        (
            "const editor appears",
            set_input(port, None, Some(constant(1.0))),
            (
                true,
                EditFacts {
                    binding_kind: true,
                    ..EditFacts::NONE
                },
            ),
        ),
        (
            "const editor leaves, as a wire commit does",
            set_input(port, Some(constant(1.0)), None),
            (
                true,
                EditFacts {
                    binding_kind: true,
                    ..EditFacts::NONE
                },
            ),
        ),
        (
            "a kept constant cleared leaves the value column",
            main(GraphStep::Input(Box::new(SetInput {
                input: port,
                state: Change {
                    from: InputState::Ghost(GhostBinding::from(constant(1.0))),
                    to: InputState::Unbound,
                },
                ties: SetTies::default(),
            }))),
            (
                true,
                EditFacts {
                    binding_kind: true,
                    ..EditFacts::NONE
                },
            ),
        ),
        (
            "a ghost wire cleared, as an unbound input, shows no constant either",
            main(GraphStep::Input(Box::new(SetInput {
                input: port,
                state: Change {
                    from: InputState::Ghost(GhostBinding::from(Binding::wire(
                        node_id,
                        PortId::new(1),
                    ))),
                    to: InputState::Unbound,
                },
                ties: SetTies::default(),
            }))),
            (true, EditFacts::NONE),
        ),
        (
            "selection",
            main(GraphStep::Selection(SetSelection {
                toggled: BTreeSet::from([ItemId::Node(node_id)]),
            })),
            (false, EditFacts::NONE),
        ),
        (
            "depth",
            main(GraphStep::Depth(SetDepth {
                node_id,
                z: Change { from: 0, to: 7 },
            })),
            (false, EditFacts::NONE),
        ),
        (
            "cache mode",
            main(GraphStep::NodeProperty(SetNodeProperty {
                node_id,
                property: PropertyChange::RuntimeCache(Change {
                    from: CacheMode::None,
                    to: CacheMode::Ram,
                }),
            })),
            (true, EditFacts::NONE),
        ),
        (
            "subscription",
            main(GraphStep::Subscription(SetSubscription {
                subscription: Subscription::new(
                    EventPort::new(node_id, PortId::new(1)),
                    NodeId::unique(),
                ),
                state: Change {
                    from: SubscriptionState::Absent,
                    to: SubscriptionState::Live,
                },
                ties: SetTies::default(),
            })),
            (true, EditFacts::NONE),
        ),
        ("tie removal", tie_removal(), (true, EditFacts::NONE)),
        (
            "subgraph insertion",
            UndoStep::Subgraph(Box::new(SetSubgraph::insertion(
                SubgraphId::from_u128(9),
                Subgraph::new("inner", Interface::default()),
                SubgraphSide::new(&Subgraph::new("inner", Interface::default()).graph),
            ))),
            (
                true,
                EditFacts {
                    node_set: true,
                    touched: Touched::Graph(GraphId::Subgraph(SubgraphId::from_u128(9))),
                    ..EditFacts::NONE
                },
            ),
        ),
        (
            "definition updated",
            UndoStep::Definition(Box::new(SetDefinition {
                id: SubgraphId::from_u128(9),
                definition: Change {
                    from: Subgraph::new("before", Interface::default()),
                    to: Subgraph::new("after", Interface::default()),
                },
                view: Change {
                    from: GraphView::default(),
                    to: GraphView::default(),
                },
            })),
            (
                true,
                EditFacts {
                    node_set: true,
                    touched: Touched::Graph(GraphId::Subgraph(SubgraphId::from_u128(9))),
                    ..EditFacts::NONE
                },
            ),
        ),
        (
            "subgraph marked shared",
            UndoStep::Side(SetSide {
                id: SubgraphId::from_u128(9),
                marks: Change {
                    from: SideMarks {
                        shared: false,
                        synced: None,
                        follows: false,
                        locked: false,
                    },
                    to: SideMarks {
                        shared: true,
                        synced: None,
                        follows: false,
                        locked: false,
                    },
                },
            }),
            (true, EditFacts::NONE),
        ),
        (
            "subgraph renamed",
            UndoStep::SubgraphName(SetSubgraphName {
                id: SubgraphId::from_u128(9),
                name: Change {
                    from: "Denoise".to_owned(),
                    to: "Clean".to_owned(),
                },
            }),
            (
                true,
                EditFacts {
                    renamed: true,
                    touched: Touched::Graph(GraphId::Subgraph(SubgraphId::from_u128(9))),
                    ..EditFacts::NONE
                },
            ),
        ),
        (
            "interface edit dropping a constant",
            interface_port_removal(),
            (
                true,
                EditFacts {
                    ports: true,
                    binding_kind: true,
                    dropped: 1,
                    touched: Touched::Several,
                    ..EditFacts::NONE
                },
            ),
        ),
    ];
    let kind = |step: &UndoStep| {
        let step = match step {
            UndoStep::Graph { step, .. } => step,
            UndoStep::Subgraph(_) => return 9,
            UndoStep::Interface(_) => return 10,
            UndoStep::Side(_) => return 11,
            UndoStep::Definition(_) => return 12,
            UndoStep::SubgraphName(_) => return 13,
        };
        match step {
            GraphStep::Node(_) => 0,
            GraphStep::Positions(_) => 1,
            GraphStep::NodeName(_) => 2,
            GraphStep::Input(_) => 3,
            GraphStep::Selection(_) => 4,
            GraphStep::Depth(_) => 5,
            GraphStep::NodeProperty(_) => 6,
            GraphStep::Subscription(_) => 7,
            GraphStep::Ties(_) => 8,
        }
    };
    let covered: BTreeSet<u8> = cases.iter().map(|(_, step, _)| kind(step)).collect();
    assert_eq!(covered.len(), 14, "a case for every kind");
    for (what, step, answers) in &cases {
        assert!(
            !step.is_noop(),
            "{what}: a degenerate step would pin nothing"
        );
        // Each graph's step also touches the graph it lands in, the main graph here.
        let (dirties, mut facts) = *answers;
        if let UndoStep::Graph { .. } = step {
            facts.touched = Touched::Graph(GraphId::Main);
        }
        assert_eq!(
            (step.dirties_document(), step.facts()),
            (dirties, facts),
            "{what}"
        );
    }
}

/// A later frame of a held gesture folds into the open step in place: each
/// member keeps its first `from` and takes the latest `to`, and a member the
/// later frame no longer carries keeps the position it last had.
#[test]
fn a_held_gesture_absorbs_its_next_frame_in_place() {
    let (a, b) = (NodeId::unique(), NodeId::unique());
    let mv = |node_id, from, to| Move {
        item: ItemId::Node(node_id),
        pos: Change { from, to },
    };
    let mut open = main(GraphStep::Positions(SetPositions {
        moves: vec![
            mv(a, Vec2::ZERO, Vec2::new(10.0, 0.0)),
            mv(b, Vec2::ONE, Vec2::new(11.0, 1.0)),
        ],
    }));
    let elsewhere = UndoStep::Graph {
        graph: GraphId::Subgraph(SubgraphId::from_u128(1)),
        step: GraphStep::Positions(SetPositions {
            moves: vec![mv(a, Vec2::new(10.0, 0.0), Vec2::new(25.0, 0.0))],
        }),
    };
    assert!(
        open.absorbs(&set_positions(
            a,
            Vec2::new(10.0, 0.0),
            Vec2::new(25.0, 0.0)
        )),
        "a frame of the same kind on the same graph"
    );
    assert!(
        !open.absorbs(&elsewhere),
        "a frame on another graph is no frame of this gesture"
    );
    // `a` dragged on to 25; `b` vanished, so the frame no longer carries it.
    open.absorb(&set_positions(
        a,
        Vec2::new(10.0, 0.0),
        Vec2::new(25.0, 0.0),
    ));
    let UndoStep::Graph {
        step: GraphStep::Positions(folded),
        ..
    } = &open
    else {
        panic!("a folded drag stays a drag: {open:?}");
    };
    let halves: Vec<_> = folded
        .moves
        .iter()
        .map(|moved| (moved.item, moved.pos.from, moved.pos.to))
        .collect();
    assert_eq!(
        halves,
        [
            (ItemId::Node(a), Vec2::ZERO, Vec2::new(25.0, 0.0)),
            (ItemId::Node(b), Vec2::ONE, Vec2::new(11.0, 1.0)),
        ]
    );
}

/// A gesture emits one kind of intent for its whole life, so a frame of
/// another kind is a caller bug, not a fold.
#[test]
#[should_panic(expected = "a gesture cannot fold")]
fn a_gesture_refuses_a_frame_of_another_kind() {
    let a = NodeId::unique();
    let mut open = set_positions(a, Vec2::ZERO, Vec2::ONE);
    open.absorb(&main(GraphStep::Depth(SetDepth {
        node_id: a,
        z: Change { from: 0, to: 1 },
    })));
}
