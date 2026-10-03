use glam::Vec2;

use super::*;
use crate::core::document::harness::DocFixture;
use crate::gui::pane::graph::harness::CanvasHarness;

/// The breaker cuts a node where the *document* says it is, not where it last
/// painted.
///
/// Both rects are one frame apart whenever something moves a node out from
/// under a live gesture — an undo, say — because `NodeCtx::pos`
/// is mirrored pre-record while the body's own arranged rect is still last
/// frame's. Driving that here: scribble over empty canvas, move the node onto
/// the scribble mid-gesture, release. The cut has to land, which it only does
/// if the probe reads the same `node_world_rect` the cull and the rubber band
/// do rather than re-deriving one off `response_for(wid::body(..))`.
#[test]
fn the_breaker_cuts_a_node_at_its_current_position_not_its_last_painted_one() {
    use palantir::PointerButton;

    // Where the scribble runs: a short vertical stroke over empty canvas, well
    // clear of where the node starts.
    const SCRIBBLE_FROM: Vec2 = Vec2::new(600.0, 600.0);
    const SCRIBBLE_TO: Vec2 = Vec2::new(600.0, 420.0);

    let mut h = CanvasHarness::new(DocFixture::probes(1));
    let node = h.node(0);
    h.prime(2);

    let body = h.node_rect(node);
    assert!(
        !body.contains(SCRIBBLE_TO),
        "the scribble must start out clear of the node, else the move proves nothing"
    );

    // Right-drag over empty canvas: the gesture latches and paints a polyline
    // nowhere near the node, so nothing is marked.
    h.ui.press_button_at(PointerButton::Right, SCRIBBLE_FROM);
    h.ui.drag_to(SCRIBBLE_TO);
    let scribbling = h.frame();
    assert!(
        scribbling.is_empty(),
        "a scribble in flight severs nothing until release: {scribbling:?}"
    );

    // Now move the node onto the scribble, centred on its far end. This frame
    // the document says the node is here while its arranged rect still says it
    // is back there — the divergence the probe has to resolve the new way.
    h.doc_mut()
        .main_view
        .item_placements
        .get_mut(&node)
        .unwrap()
        .pos = SCRIBBLE_TO - Vec2::new(body.size.w, body.size.h) * 0.5;
    h.frame();

    h.ui.release_button(PointerButton::Right);
    // The harness carries the pane assertion: a cut commits against the pane
    // the scribble ran on.
    let released = h.frame();
    assert!(
        matches!(released[..], [GraphIntent::RemoveNode { node_id }] if node_id == node),
        "the release cuts the node the scribble now crosses: {released:?}"
    );
}
