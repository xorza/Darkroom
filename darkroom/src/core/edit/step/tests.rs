//! The per-step predicates the undo stack and the frame pipeline read off a
//! step.
//!
//! The frame-cost questions — whether a step dirties the document, whether
//! it strands the canvas caches, whether it retypes an output — are swept
//! over one representative of *every* kind, so a kind whose answer is wrong
//! shows up here rather than as a save prompt that never fires, a drag that
//! pays for two record passes, or a wire drawn with a stale type.

use std::collections::BTreeSet;

use glam::Vec2;
use scenarium::{Binding, CacheMode, ConstValue, InputPort, NodeId, Subscription};

use crate::core::document::Viewport;
use crate::core::document::harness::DocFixture;
use crate::core::edit::step::change::Change;
use crate::core::edit::step::move_selection::{Move, MoveSelection};
use crate::core::edit::step::node_presence::{NodePresence, NodeState};
use crate::core::edit::step::raise::Raise;
use crate::core::edit::step::rename_node::RenameNode;
use crate::core::edit::step::set_input::SetInput;
use crate::core::edit::step::set_node_property::{NodeProperty, SetNodeProperty};
use crate::core::edit::step::set_selection::SetSelection;
use crate::core::edit::step::set_subscription::SetSubscription;
use crate::core::edit::step::set_viewport::SetViewport;
use crate::core::edit::step::undo_step::UndoStep;

fn viewport(pan: Vec2, zoom: f32) -> Viewport {
    Viewport { pan, zoom }
}

fn move_step(key: NodeId, from: Vec2, to: Vec2) -> UndoStep {
    UndoStep::MoveSelection(MoveSelection {
        moves: vec![Move {
            key,
            pos: Change { from, to },
        }],
    })
}

fn set_input(input: InputPort, from: Option<Binding>, to: Option<Binding>) -> UndoStep {
    UndoStep::SetInput(SetInput {
        input,
        binding: Change { from, to },
    })
}

fn cst(v: f64) -> Binding {
    Binding::Const(ConstValue::Float(v))
}

fn subscription(emitter: NodeId, subscriber: NodeId, from: bool, to: bool) -> UndoStep {
    UndoStep::SetSubscription(SetSubscription {
        subscription: Subscription {
            emitter,
            event_idx: 0,
            subscriber,
        },
        subscribed: Change { from, to },
    })
}

fn node_property(node_id: NodeId, from: CacheMode, to: CacheMode) -> UndoStep {
    UndoStep::SetNodeProperty(SetNodeProperty {
        node_id,
        property: Change {
            from: NodeProperty::RuntimeCache(from),
            to: NodeProperty::RuntimeCache(to),
        },
    })
}

/// A removal of a real node — the one kind that can only be built by reading a
/// document, since it carries everything the graph held about the node.
fn node_presence() -> UndoStep {
    let mut fixture = DocFixture::default();
    let node_id = fixture.stub_at(Vec2::ZERO);
    let state = NodeState::capture(&fixture.doc, node_id).expect("the fixture placed it");
    UndoStep::NodePresence(Box::new(NodePresence::removal(state)))
}

/// The exit prompt's split: camera, selection and stacking are navigation and
/// must not flip the unsaved flag; graph data and node layout must.
#[test]
fn dirties_document_splits_edits_from_navigation() {
    let node_id = NodeId::unique();
    let navigation = [
        UndoStep::SetSelection(SetSelection {
            selection: Change {
                from: BTreeSet::new(),
                to: BTreeSet::from([node_id]),
            },
        }),
        UndoStep::SetViewport(SetViewport {
            viewport: Change {
                from: viewport(Vec2::ZERO, 1.0),
                to: viewport(Vec2::new(10.0, 20.0), 2.0),
            },
        }),
        UndoStep::Raise(Raise {
            key: node_id,
            z: Change { from: 0, to: 7 },
        }),
    ];
    for step in &navigation {
        assert!(
            !step.dirties_document(),
            "navigation step must not dirty: {step:?}",
        );
    }

    let content = [
        node_presence(),
        UndoStep::RenameNode(RenameNode {
            node_id,
            name: Change {
                from: "a".into(),
                to: "b".into(),
            },
        }),
        move_step(node_id, Vec2::ZERO, Vec2::new(5.0, 5.0)),
        set_input(InputPort::new(node_id, 0), None, Some(cst(1.0))),
        node_property(node_id, CacheMode::None, CacheMode::Ram),
        subscription(node_id, NodeId::unique(), false, true),
    ];
    for step in &content {
        assert!(step.dirties_document(), "content step must dirty: {step:?}");
    }
}

/// Only a node coming or going, or a binding changing, can move a wildcard
/// output's resolved type; every other kind leaves the type table as it was,
/// so a drag frame resolves nothing.
#[test]
fn retypes_outputs_splits_wiring_from_everything_else() {
    let node_id = NodeId::unique();
    let every_kind = [
        (node_presence(), true),
        (
            set_input(InputPort::new(node_id, 0), None, Some(cst(1.0))),
            true,
        ),
        (move_step(node_id, Vec2::ZERO, Vec2::new(5.0, 5.0)), false),
        (
            UndoStep::RenameNode(RenameNode {
                node_id,
                name: Change {
                    from: "a".into(),
                    to: "b".into(),
                },
            }),
            false,
        ),
        (
            UndoStep::SetSelection(SetSelection {
                selection: Change {
                    from: BTreeSet::new(),
                    to: BTreeSet::from([node_id]),
                },
            }),
            false,
        ),
        (
            UndoStep::Raise(Raise {
                key: node_id,
                z: Change { from: 0, to: 7 },
            }),
            false,
        ),
        (
            node_property(node_id, CacheMode::None, CacheMode::Ram),
            false,
        ),
        (
            UndoStep::SetViewport(SetViewport {
                viewport: Change {
                    from: viewport(Vec2::ZERO, 1.0),
                    to: viewport(Vec2::new(10.0, 20.0), 2.0),
                },
            }),
            false,
        ),
        (subscription(node_id, NodeId::unique(), false, true), false),
    ];
    for (step, retypes) in &every_kind {
        assert_eq!(step.retypes_outputs(), *retypes, "{step:?}");
    }
}

/// A true arm here costs a whole extra record pass, and the step most at risk
/// — a node drag — emits one per *gesture frame*, so a spurious true doubles
/// the editor pipeline for the length of the drag. The split under test: only
/// a step that changes a widget's measured size, or introduces a node with no
/// cached port offsets, may return true.
#[test]
fn invalidates_cached_geometry_splits_resizes_from_moves() {
    let node_id = NodeId::unique();
    let port = InputPort::new(node_id, 0);

    // Nothing remeasures: a port center is `node.pos + cached offset`, and
    // every one of these leaves that offset valid.
    let moves = [
        // The node drag. Emits one step per gesture frame, drains pre-record,
        // and Pass A already arranges at the cursor.
        move_step(node_id, Vec2::ZERO, Vec2::new(5.0, 5.0)),
        UndoStep::SetViewport(SetViewport {
            viewport: Change {
                from: viewport(Vec2::ZERO, 1.0),
                to: viewport(Vec2::new(10.0, 20.0), 2.0),
            },
        }),
        UndoStep::SetSelection(SetSelection {
            selection: Change {
                from: BTreeSet::new(),
                to: BTreeSet::from([node_id]),
            },
        }),
        UndoStep::Raise(Raise {
            key: node_id,
            z: Change { from: 0, to: 7 },
        }),
        // Value-only: the editor stays present at its `Fixed` size.
        set_input(port, Some(cst(1.0)), Some(cst(2.0))),
        // A dimmed body and a filled badge keep the same rect...
        node_property(node_id, CacheMode::None, CacheMode::Ram),
        // ...and an event wire paints between glyphs that are already there.
        subscription(node_id, NodeId::unique(), false, true),
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

    // Each of these changes a measured size: a wider title reflows the header,
    // and the inline const editor appearing or leaving shifts every port row
    // below it.
    let resizes = [
        UndoStep::RenameNode(RenameNode {
            node_id,
            name: Change {
                from: "a".into(),
                to: "a-much-longer-title".into(),
            },
        }),
        set_input(port, None, Some(cst(1.0))),
        // ...and removing it is the connection commit, the case Pass B has
        // always existed for.
        set_input(port, Some(cst(1.0)), None),
        // A node arriving — or coming back on an undo — has no cached port
        // offsets for its wires to anchor to.
        node_presence(),
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

/// A later frame of a held gesture folds into the open step in place: each
/// member keeps its first `from` and takes the latest `to`, and a member the
/// later frame no longer carries keeps the position it last had.
#[test]
fn a_held_gesture_absorbs_its_next_frame_in_place() {
    let (a, b) = (NodeId::unique(), NodeId::unique());
    let mv = |key, from, to| Move {
        key,
        pos: Change { from, to },
    };
    let mut open = UndoStep::MoveSelection(MoveSelection {
        moves: vec![
            mv(a, Vec2::ZERO, Vec2::new(10.0, 0.0)),
            mv(b, Vec2::ONE, Vec2::new(11.0, 1.0)),
        ],
    });
    // `a` dragged on to 25; `b` vanished, so the frame no longer carries it.
    open.absorb(&move_step(a, Vec2::new(10.0, 0.0), Vec2::new(25.0, 0.0)));
    let UndoStep::MoveSelection(folded) = &open else {
        panic!("a folded drag stays a drag: {open:?}");
    };
    let halves: Vec<_> = folded
        .moves
        .iter()
        .map(|moved| (moved.key, moved.pos.from, moved.pos.to))
        .collect();
    assert_eq!(
        halves,
        [
            (a, Vec2::ZERO, Vec2::new(25.0, 0.0)),
            (b, Vec2::ONE, Vec2::new(11.0, 1.0)),
        ]
    );

    let camera = |from, to| {
        UndoStep::SetViewport(SetViewport {
            viewport: Change { from, to },
        })
    };
    let (start, mid, end) = (
        viewport(Vec2::ZERO, 1.0),
        viewport(Vec2::new(5.0, 0.0), 1.0),
        viewport(Vec2::new(5.0, 0.0), 2.0),
    );
    let mut open = camera(start, mid);
    open.absorb(&camera(mid, end));
    let UndoStep::SetViewport(folded) = &open else {
        panic!("a folded camera move stays one: {open:?}");
    };
    assert_eq!(
        folded.viewport,
        Change {
            from: start,
            to: end
        }
    );
}

/// A gesture emits one kind of intent for its whole life, so a frame of
/// another kind is a caller bug, not a fold.
#[test]
#[should_panic(expected = "a gesture cannot fold")]
fn a_gesture_refuses_a_frame_of_another_kind() {
    let a = NodeId::unique();
    let mut open = move_step(a, Vec2::ZERO, Vec2::ONE);
    open.absorb(&UndoStep::Raise(Raise {
        key: a,
        z: Change { from: 0, to: 1 },
    }));
}

/// The camera compares with a tolerance rather than for equality: a pan of a
/// thousandth of a pixel is the same camera, and recording it would put a
/// Ctrl+Z between the user and their last real edit.
#[test]
fn viewport_noop_is_measured_not_exact() {
    let same = |from, to| {
        UndoStep::SetViewport(SetViewport {
            viewport: Change { from, to },
        })
        .is_noop()
    };
    let base = viewport(Vec2::new(3.0, 4.0), 1.5);

    assert!(same(base, base), "an unmoved camera is a no-op");
    // Just inside the 1e-4 threshold, on each axis in turn.
    assert!(same(base, viewport(base.pan + Vec2::new(5e-5, 0.0), 1.5)));
    assert!(same(base, viewport(base.pan, 1.5 + 5e-5)));
    // ...and just outside it.
    assert!(!same(base, viewport(base.pan + Vec2::new(2e-4, 0.0), 1.5)));
    assert!(!same(base, viewport(base.pan, 1.5 + 2e-4)));
}
