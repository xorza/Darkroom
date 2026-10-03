use glam::Vec2;
use palantir::{Key, PointerButton};

use crate::core::document::internals::DocFixture;
use crate::core::edit::graph_intent::GraphIntent;
use crate::core::edit::graph_intent::drag_start::DragStart;
use crate::gui::pane::graph::internals::CanvasHarness;

/// A drag on a node body moves that node, by the pointer's travel.
///
/// The latch is read where the body is drawn: `NodeWidget::show` walks the
/// node's own `drag_handles` for a fresh press, and later frames' `prepass`
/// turns that handle's `drag_delta` into a `MoveSelection`. So this drives a
/// real press-and-travel through the harness — nothing else in the suite
/// latches a body drag, and a latch that silently stopped firing would leave
/// every node unmovable with the whole rest of the canvas green.
#[test]
fn a_body_drag_moves_the_node_by_the_pointers_travel() {
    let mut h = CanvasHarness::new(DocFixture::probes(2));
    let (dragged, bystander) = (h.node(0), h.node(1));
    let start = h.doc().main_view.item_placements[&dragged].pos;
    h.prime(2);

    // Press the body, then travel past the drag threshold. The sweep sees
    // the latch on the frame after the travel; the record consumes it.
    let grab = h.node_center(dragged);
    h.ui.press_at(grab);
    h.frame();
    let travel = Vec2::new(37.0, -21.0);
    h.ui.drag_to(grab + travel);
    h.frame();

    // Next frame, `NodeUI::prepass` advances the anchor the record latched.
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
                node: dragged,
                pos: start
            }][..],
            travel
        ),
        "the drag moves only the grabbed node, from its start by the travel"
    );
    assert!(
        !members.iter().any(|member| member.node == bystander),
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
    let mut h = CanvasHarness::new(DocFixture::probes(2));
    let dragged = h.node(0);
    h.doc_mut().main_view.selected = [dragged].into_iter().collect();
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
        } => Some((&members[..], *offset)),
        _ => None,
    })
}
