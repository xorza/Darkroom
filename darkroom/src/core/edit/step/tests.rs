//! The per-step predicates the undo history and the frame pipeline read off a
//! step.
//!
//! The frame-cost questions are swept over one representative of every kind,
//! and a held gesture's fold is checked on the kinds that fold.

use std::collections::BTreeSet;

use glam::Vec2;
use scenarium::{
    Binding, CacheMode, ConstValue, EventPort, InputPort, NodeId, OutputPort, Subscription,
};

use crate::core::document::internals::DocumentFixture;
use crate::core::document::item_id::ItemId;
use crate::core::edit::edit_facts::EditFacts;
use crate::core::edit::graph_intent::GraphIntent;
use crate::core::edit::step::change::Change;
use crate::core::edit::step::set_depth::SetDepth;
use crate::core::edit::step::set_input::SetInput;
use crate::core::edit::step::set_node::{NodeEntry, SetNode};
use crate::core::edit::step::set_node_name::SetNodeName;
use crate::core::edit::step::set_node_property::{PropertyChange, SetNodeProperty};
use crate::core::edit::step::set_positions::{Move, SetPositions};
use crate::core::edit::step::set_selection::SetSelection;
use crate::core::edit::step::set_subscription::SetSubscription;
use crate::core::edit::step::set_ties::SetTies;
use crate::core::edit::step::undo_step::UndoStep;

fn set_positions(node_id: NodeId, from: Vec2, to: Vec2) -> UndoStep {
    UndoStep::Positions(SetPositions {
        moves: vec![Move {
            item: ItemId::Node(node_id),
            pos: Change { from, to },
        }],
    })
}

fn set_input(input: InputPort, from: Option<Binding>, to: Option<Binding>) -> UndoStep {
    UndoStep::Input(SetInput {
        input,
        binding: Change { from, to },
        ties: SetTies::default(),
    })
}

fn constant(v: f64) -> Binding {
    Binding::Const(ConstValue::Float(v))
}

fn renamed(node_id: NodeId, from: &str, to: &str) -> UndoStep {
    UndoStep::NodeName(SetNodeName {
        node_id,
        name: Change {
            from: from.into(),
            to: to.into(),
        },
    })
}

/// A removal of a stub — the one kind that can only be built by reading a
/// document, since it carries everything the document held about the node.
fn node_removal() -> UndoStep {
    let mut fixture = DocumentFixture::default();
    let node_id = fixture.stub_at(Vec2::ZERO);
    let entry = NodeEntry::capture(&fixture.document, node_id).expect("the fixture placed it");
    UndoStep::Node(Box::new(SetNode::removal(entry, SetTies::default())))
}

/// A removal of a tie, read off a document that holds one over a wire.
fn tie_removal() -> UndoStep {
    let mut fixture = DocumentFixture::default();
    let [a, b] = [0.0, 200.0].map(|x| fixture.node_at(Vec2::new(x, 0.0)));
    let wire = fixture.wire(OutputPort::new(a, 0), InputPort::new(b, 0));
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
    let port = InputPort::new(node_id, 0);
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
            "selection",
            UndoStep::Selection(SetSelection {
                toggled: BTreeSet::from([ItemId::Node(node_id)]),
            }),
            (false, EditFacts::NONE),
        ),
        (
            "depth",
            UndoStep::Depth(SetDepth {
                node_id,
                z: Change { from: 0, to: 7 },
            }),
            (false, EditFacts::NONE),
        ),
        (
            "cache mode",
            UndoStep::NodeProperty(SetNodeProperty {
                node_id,
                property: PropertyChange::RuntimeCache(Change {
                    from: CacheMode::None,
                    to: CacheMode::Ram,
                }),
            }),
            (true, EditFacts::NONE),
        ),
        (
            "subscription",
            UndoStep::Subscription(SetSubscription {
                subscription: Subscription::new(EventPort::new(node_id, 0), NodeId::unique()),
                subscribed: Change {
                    from: false,
                    to: true,
                },
                ties: SetTies::default(),
            }),
            (true, EditFacts::NONE),
        ),
        ("tie removal", tie_removal(), (true, EditFacts::NONE)),
    ];
    let kind = |step: &UndoStep| match step {
        UndoStep::Node(_) => 0,
        UndoStep::Positions(_) => 1,
        UndoStep::NodeName(_) => 2,
        UndoStep::Input(_) => 3,
        UndoStep::Selection(_) => 4,
        UndoStep::Depth(_) => 5,
        UndoStep::NodeProperty(_) => 6,
        UndoStep::Subscription(_) => 7,
        UndoStep::Ties(_) => 8,
    };
    let covered: BTreeSet<u8> = cases.iter().map(|(_, step, _)| kind(step)).collect();
    assert_eq!(covered.len(), 9, "a case for every kind");
    for (what, step, answers) in &cases {
        assert!(
            !step.is_noop(),
            "{what}: a degenerate step would pin nothing"
        );
        assert_eq!((step.dirties_document(), step.facts()), *answers, "{what}");
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
    let mut open = UndoStep::Positions(SetPositions {
        moves: vec![
            mv(a, Vec2::ZERO, Vec2::new(10.0, 0.0)),
            mv(b, Vec2::ONE, Vec2::new(11.0, 1.0)),
        ],
    });
    // `a` dragged on to 25; `b` vanished, so the frame no longer carries it.
    open.absorb(&set_positions(
        a,
        Vec2::new(10.0, 0.0),
        Vec2::new(25.0, 0.0),
    ));
    let UndoStep::Positions(folded) = &open else {
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
    open.absorb(&UndoStep::Depth(SetDepth {
        node_id: a,
        z: Change { from: 0, to: 1 },
    }));
}
