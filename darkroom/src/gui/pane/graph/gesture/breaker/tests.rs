use glam::Vec2;
use scenarium::InputPort;

use super::*;
use crate::core::document::harness::DocFixture;
use crate::core::edit::graph_intent::GraphIntent;
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

/// A scribble started at `p`, the way [`BreakerUI::apply`] starts one — the
/// unit under test in everything below, which is about the polyline rather
/// than about the gesture that drives it.
fn scribble_at(p: Vec2) -> Scribble {
    let mut s = Scribble::default();
    s.restart(p);
    s
}

#[test]
fn begin_frame_clears_every_broken_collection() {
    // Regression: `broken_subscriptions`/`broken_pins` used to have no
    // per-frame clear at all (each renderer was responsible for its own
    // field, and two of the four forgot), so a port/wire crossed once
    // mid-drag stayed marked even after the scribble moved away —
    // over-committing severs on release. `begin_frame` is now the one
    // place that clears all three.
    let mut b = scribble_at(Vec2::ZERO);
    let node = NodeId::from_u128(1);
    b.broken.push(InputPort::new(node, 0));
    b.broken_nodes.push(node);
    b.broken_subscriptions.push(Subscription {
        emitter: node,
        event_idx: 0,
        subscriber: node,
    });

    b.begin_frame();

    assert!(b.broken.is_empty());
    assert!(b.broken_nodes.is_empty());
    assert!(b.broken_subscriptions.is_empty());
}

#[test]
fn add_point_skips_short_segments() {
    // Samples below MIN_POINT_DISTANCE are dropped — a slow drag
    // that crawls 1px/frame must not accumulate one point per frame.
    let mut b = scribble_at(Vec2::ZERO);
    b.add_point(Vec2::new(1.0, 0.0));
    b.add_point(Vec2::new(2.0, 0.0));
    b.add_point(Vec2::new(3.0, 0.0));
    assert_eq!(b.points.len(), 1, "sub-4px samples must be dropped");
    b.add_point(Vec2::new(10.0, 0.0));
    assert_eq!(b.points.len(), 2);
}

#[test]
fn add_point_caps_total_length() {
    // Past MAX_BREAKER_LENGTH the last segment is clamped and
    // further pushes are no-ops. Hand-computed: starting at 0 and
    // pushing (3000, 0) has seg = 3000 > remaining = 2000, so t =
    // 2000/3000 and the appended point lands at exactly
    // (2000, 0) — the cap.
    let mut b = scribble_at(Vec2::ZERO);
    b.add_point(Vec2::new(3000.0, 0.0));
    assert_eq!(b.points.len(), 2);
    assert!((b.points[1].x - MAX_BREAKER_LENGTH).abs() < 1e-4);
    let before = b.points.len();
    b.add_point(Vec2::new(4000.0, 0.0));
    assert_eq!(b.points.len(), before, "no append past cap");
}

#[test]
fn intersects_cubic_diagonal_through_straight_wire() {
    // Straight horizontal cubic from (0,0) to (100,0). A breaker
    // segment crossing it transversely must register. Vertical
    // breaker at x=50, y from -10 to +10 — proper crossing at
    // (50, 0), nowhere near a cubic endpoint (which would be a
    // degenerate "touch at vertex" the strict-crossing test
    // intentionally rejects).
    let mut b = scribble_at(Vec2::new(50.0, -10.0));
    b.add_point(Vec2::new(50.0, 10.0));
    assert!(b.intersects_cubic(
        Vec2::new(0.0, 0.0),
        Vec2::new(33.0, 0.0),
        Vec2::new(66.0, 0.0),
        Vec2::new(100.0, 0.0),
    ));
}

#[test]
fn intersects_cubic_misses_parallel_polyline() {
    // Breaker runs parallel to the wire well below it — no crossing.
    let mut b = scribble_at(Vec2::new(0.0, 50.0));
    b.add_point(Vec2::new(100.0, 50.0));
    assert!(!b.intersects_cubic(
        Vec2::new(0.0, 0.0),
        Vec2::new(33.0, 0.0),
        Vec2::new(66.0, 0.0),
        Vec2::new(100.0, 0.0),
    ));
}

#[test]
fn intersects_cubic_empty_breaker_is_false() {
    // Single-point breaker (no segments yet) can't intersect.
    let b = scribble_at(Vec2::ZERO);
    assert!(!b.intersects_cubic(
        Vec2::ZERO,
        Vec2::new(1.0, 0.0),
        Vec2::new(2.0, 0.0),
        Vec2::new(3.0, 0.0),
    ));
}

/// The point of keeping the scribble beside the slot rather than inside it: a
/// finished gesture leaves its buffers for the next one instead of returning
/// them to the allocator.
///
/// 49 samples 10 units apart — clear of `MIN_POINT_DISTANCE` (4) so every one
/// is kept, and 490 total, well under `MAX_BREAKER_LENGTH` (2000) so none is
/// clamped away. That gives 50 points including the start, so the buffer has
/// grown past any small-vec default by the time the gesture ends.
#[test]
fn restarting_a_scribble_keeps_the_buffer_the_last_one_grew() {
    let mut s = scribble_at(Vec2::ZERO);
    for i in 1..50 {
        s.add_point(Vec2::new(i as f32 * 10.0, 0.0));
    }
    assert_eq!(s.points.len(), 50, "every sample cleared both thresholds");
    let grown = s.points.capacity();
    assert!(grown >= 50, "the buffer holds what it collected: {grown}");

    s.restart(Vec2::new(7.0, 7.0));
    assert_eq!(
        s.points.capacity(),
        grown,
        "a fresh gesture reuses the last one's allocation"
    );
    assert_eq!(
        s.points.as_slice(),
        &[Vec2::new(7.0, 7.0)],
        "and starts a genuinely new polyline in it"
    );
    assert_eq!(s.length, 0.0, "with the length accumulator back to zero");
}
