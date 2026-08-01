use glam::Vec2;

use crate::core::document::harness::DocFixture;
use crate::core::edit::intent::types::GraphIntent;
use crate::gui::pane::graph::harness::CanvasHarness;

/// A drag on a node body moves that node, by the pointer's travel.
///
/// The latch is read where the body is drawn: `NodeUI::draw_one` walks the
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
    let moves = intents
        .iter()
        .find_map(|intent| match intent {
            GraphIntent::MoveSelection { grabbed, moves } => Some((*grabbed, moves)),
            _ => None,
        })
        .unwrap_or_else(|| panic!("a body drag must emit a MoveSelection: {intents:?}"));
    assert_eq!(moves.0, dragged, "the grabbed node is the one pressed");
    // Target = press-frame position + cumulative travel, so the node lands
    // exactly where the pointer took it — and the untouched node stays out
    // of the batch, since the grab selected only the node under it.
    assert_eq!(
        moves.1.as_slice(),
        &[(dragged, start + travel)],
        "the drag moves only the grabbed node, to its start plus the travel"
    );
    assert!(
        !moves.1.iter().any(|(id, _)| *id == bystander),
        "an unselected neighbour is not dragged along"
    );
}
