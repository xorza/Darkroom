use glam::Vec2;
use palantir::{Key, PointerButton};
use scenarium::{InputPort, OutputPort};

use crate::core::document::internals::DocumentFixture;
use crate::core::document::item_id::ItemId;
use crate::core::edit::graph_intent::GraphIntent;
use crate::core::edit::graph_intent::drag_start::DragStart;
use crate::gui::pane::graph_ui::gesture::group_drag;
use crate::gui::pane::graph_ui::internals::CanvasHarness;

/// A drag on a node body moves that node, by the pointer's travel.
///
/// The latch is read where the body is drawn: `NodeUi::draw_node` walks the
/// node's own `drag_handles` for a fresh press, and later frames' `prepass`
/// turns that handle's `drag_delta` into a `MoveSelection`. So this drives a
/// real press-and-travel through the harness — nothing else in the suite
/// latches a body drag, and a latch that silently stopped firing would leave
/// every node unmovable with the whole rest of the canvas green.
#[test]
fn a_body_drag_moves_the_node_by_the_pointers_travel() {
    let mut h = CanvasHarness::new(DocumentFixture::probes(2));
    let (dragged, bystander) = (h.node(0), h.node(1));
    let start = h.document().main_view.node_placements[&dragged].pos;
    h.prime(2);

    // Press the body, then travel past the drag threshold. The sweep sees
    // the latch on the frame after the travel; the record consumes it.
    let grab = h.node_center(dragged);
    h.ui.press_at(grab);
    h.frame();
    let travel = Vec2::new(37.0, -21.0);
    h.ui.drag_to(grab + travel);
    h.frame();

    // Next frame, `NodeUi::prepass` advances the anchor the record latched.
    let intents = h.frame();
    let (members, offset) = moved(&intents)
        .unwrap_or_else(|| panic!("a body drag must emit a MoveSelection: {intents:?}"));
    // Target = press-frame position + cumulative travel, so the node lands
    // exactly where the pointer took it — and the untouched node stays out
    // of the batch, since the grab selected only the node under it.
    assert_eq!(
        (members, offset),
        (
            &[DragStart {
                item: ItemId::Node(dragged),
                pos: start
            }][..],
            travel
        ),
        "the drag moves only the grabbed node, from its start by the travel"
    );
    assert!(
        !members
            .iter()
            .any(|member| member.item == ItemId::Node(bystander)),
        "an unselected neighbour is not dragged along"
    );

    // Esc puts the node back where the drag latched it and ends the drag:
    // the pointer still held moves nothing after.
    h.ui.key(Key::Escape);
    let cancelled = h.frame();
    assert_eq!(
        moved(&cancelled).map(|(_, offset)| offset),
        Some(Vec2::ZERO),
        "a cancel returns the members to their start: {cancelled:?}"
    );
    h.ui.drag_to(grab + travel * 2.0);
    let after = h.frame();
    assert_eq!(
        moved(&after),
        None,
        "a cancelled drag moves nothing: {after:?}"
    );
}

/// Esc cancels what is in flight, and deselects only when nothing was: a
/// first Esc during a drag of the selection ends the drag and keeps the
/// selection, and a second, with nothing left in flight, clears it.
#[test]
fn esc_cancels_a_drag_before_it_deselects() {
    let mut h = CanvasHarness::new(DocumentFixture::probes(2));
    let dragged = h.node(0);
    h.document_mut().main_view.selected = [dragged].map(ItemId::Node).into_iter().collect();
    h.prime(2);

    let grab = h.node_center(dragged);
    h.ui.press_at(grab);
    h.frame();
    h.ui.drag_to(grab + Vec2::new(40.0, 0.0));
    h.frame();
    h.frame();

    h.ui.key(Key::Escape);
    let first = h.frame();
    assert!(
        matches!(first[..], [GraphIntent::MoveSelection { offset, .. }] if offset == Vec2::ZERO),
        "the first Esc only puts the drag back: {first:?}"
    );
    h.ui.release_button(PointerButton::Left);
    h.frame();

    h.ui.key(Key::Escape);
    let second = h.frame();
    assert!(
        matches!(second[..], [GraphIntent::SetSelection { ref to }] if to.is_empty()),
        "the second Esc deselects: {second:?}"
    );
}

/// The members and offset of the frame's one `MoveSelection`, if it has one.
fn moved(intents: &[GraphIntent]) -> Option<(&[DragStart], Vec2)> {
    intents.iter().find_map(|intent| match intent {
        GraphIntent::MoveSelection {
            members, offset, ..
        } => Some((&**members, *offset)),
        _ => None,
    })
}

/// A drag of `a` and `b` takes `inner`, whose one wire runs `a → b`, after
/// them at its position, and leaves `outer`, which also holds `a → c` to `c`
/// outside the drag. A selected tie among the members is not added twice.
#[test]
fn a_drag_of_two_nodes_takes_the_tie_between_them() {
    let mut fixture = DocumentFixture::default();
    let [a, b, c] = [0.0, 220.0, 440.0].map(|x| fixture.node_at(Vec2::new(x, 0.0)));
    let ab = fixture.wire(OutputPort::new(a, 0), InputPort::new(b, 0));
    let ac = fixture.wire(OutputPort::new(a, 0), InputPort::new(c, 0));
    let inner = fixture.tie(Vec2::new(110.0, 30.0), [ab]);
    fixture.tie(Vec2::new(200.0, 60.0), [ab, ac]);
    let view = &fixture.document.main_view;
    let start = |item, pos| DragStart { item, pos };
    let nodes = vec![
        start(ItemId::Node(a), Vec2::ZERO),
        start(ItemId::Node(b), Vec2::new(220.0, 0.0)),
    ];
    let inner_start = start(ItemId::Tie(inner), Vec2::new(110.0, 30.0));
    assert_eq!(
        *group_drag::with_inner_ties(view, nodes.clone()),
        [nodes[0], nodes[1], inner_start]
    );
    let with_selected_tie = vec![nodes[0], inner_start, nodes[1]];
    assert_eq!(
        *group_drag::with_inner_ties(view, with_selected_tie.clone()),
        *with_selected_tie,
        "a member already in the drag stays where it is, once"
    );
}
