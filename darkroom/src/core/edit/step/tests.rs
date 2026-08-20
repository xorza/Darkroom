//! The per-step predicates the undo stack and the frame pipeline read off a
//! step.
//!
//! The two the trait leaves no default for — whether a step dirties the
//! document and whether it strands the canvas caches — are swept over one
//! representative of *every* kind, so a kind whose answer is wrong shows up
//! here rather than as a save prompt that never fires or a drag that pays for
//! two record passes.

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
        grabbed: key,
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

fn cst(v: f64) -> Option<Binding> {
    Some(Binding::Const(ConstValue::Float(v)))
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
    UndoStep::NodePresence(NodePresence::removal(state))
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
        set_input(InputPort::new(node_id, 0), None, cst(1.0)),
        node_property(node_id, CacheMode::None, CacheMode::Ram),
        subscription(node_id, NodeId::unique(), false, true),
    ];
    for step in &content {
        assert!(step.dirties_document(), "content step must dirty: {step:?}",);
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
        set_input(port, cst(1.0), cst(2.0)),
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
        set_input(port, None, cst(1.0)),
        // ...and removing it is the connection commit, the case Pass B has
        // always existed for.
        set_input(port, cst(1.0), None),
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

/// Only the two kinds a pointer can hold open carry a gesture key, and each
/// answers `coalesce` for its own variant and nothing else — the pairing the
/// action stack relies on when it folds an entry in place.
#[test]
fn only_held_gestures_coalesce() {
    let (a, b) = (NodeId::unique(), NodeId::unique());
    let rename = UndoStep::RenameNode(RenameNode {
        node_id: a,
        name: Change {
            from: "a".into(),
            to: "b".into(),
        },
    });
    assert!(rename.gesture_key().is_none(), "a rename is its own entry");
    assert!(rename.coalesce(&rename).is_none());

    // Two frames of the same drag fold into one step spanning both.
    let first = move_step(a, Vec2::ZERO, Vec2::new(10.0, 0.0));
    let second = move_step(a, Vec2::new(10.0, 0.0), Vec2::new(25.0, 0.0));
    assert_eq!(first.gesture_key(), second.gesture_key());
    let folded = first.coalesce(&second).expect("one drag, one entry");
    let UndoStep::MoveSelection(folded) = &folded else {
        panic!("a folded drag stays a drag: {folded:?}");
    };
    assert_eq!(folded.moves.len(), 1);
    assert_eq!(
        (folded.moves[0].pos.from, folded.moves[0].pos.to),
        (Vec2::ZERO, Vec2::new(25.0, 0.0)),
        "the fold keeps the first `from` and takes the last `to`"
    );

    // A different grabbed member is a different gesture, and folds nothing.
    let other = move_step(b, Vec2::ZERO, Vec2::new(1.0, 0.0));
    assert_ne!(first.gesture_key(), other.gesture_key());
    // Kinds never fold across variants, whatever the stack asks.
    assert!(first.coalesce(&rename).is_none());
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
