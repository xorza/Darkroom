use glam::{UVec2, Vec2};
use palantir::internals::UiHarness;
use palantir::{Configure, Panel, Sizing, Ui};
use scenarium::{InputPort, OutputTypes};

use crate::core::document::Document;
use crate::core::document::harness::{one_func_library, spread};
use crate::core::edit::intent::sink::Intents;
use crate::core::edit::intent::types::GraphIntent;
use crate::gui::pane::graph::GraphUI;
use crate::gui::pane::graph::harness::*;
use crate::gui::pane::graph::node::node_widget_id;
use crate::gui::state::run_state::RunState;
use crate::gui::theme::Theme;

/// The breaker cuts a node where the *document* says it is, not where it last
/// painted.
///
/// Both rects are one frame apart whenever something moves a node out from
/// under a live gesture — an undo, say — because `SceneNode::pos`
/// is mirrored pre-record while the body's own arranged rect is still last
/// frame's. Driving that here: scribble over empty canvas, move the node onto
/// the scribble mid-gesture, release. The cut has to land, which it only does
/// if the probe reads the same `node_world_rect` the cull and the rubber band
/// do rather than re-deriving one off `response_for(node_widget_id(..))`.
#[test]
fn the_breaker_cuts_a_node_at_its_current_position_not_its_last_painted_one() {
    use palantir::PointerButton;

    use crate::core::document::GraphView;

    // Where the scribble runs: a short vertical stroke over empty canvas, well
    // clear of where the node starts.
    const SCRIBBLE_FROM: Vec2 = Vec2::new(600.0, 600.0);
    const SCRIBBLE_TO: Vec2 = Vec2::new(600.0, 420.0);

    let library = one_func_library();
    let probe = library.by_name("probe").expect("just added").clone();

    let mut doc = Document::default();
    let node = doc.graph.add_func_node(&probe);
    doc.main_view = GraphView::for_graph(&doc.graph);
    spread(&mut doc.main_view);

    let theme = Theme::default();
    let run_state = RunState::default();
    let mut harness = UiHarness::new(UVec2::new(1200, 800));
    let mut graph_ui = GraphUI::default();

    // `doc` is a parameter, not a capture, so it can be edited between frames
    // the way an undo would edit the document under a running gesture.
    let draw = |ui: &mut Ui, graph_ui: &mut GraphUI, doc: &Document| {
        let ctx = app(&theme, &library, &run_state);
        let mut intents = Intents::default();
        let mut types = OutputTypes::default();
        let graph_ctx = graph_ctx_for(ctx, doc, &mut types);
        graph_ui.prepass(ui, graph_ctx, &mut intents);
        Panel::vstack()
            .id_salt("pane")
            .size((Sizing::FILL, Sizing::FILL))
            .show(ui, |ui| {
                graph_ui.draw(ui, graph_ctx, &mut intents);
            });
        intents.drain().collect::<Vec<_>>()
    };

    harness.frame(|ui| {
        draw(ui, &mut graph_ui, &doc);
    });
    harness.frame(|ui| {
        draw(ui, &mut graph_ui, &doc);
    });
    let body = harness
        .rect(node_widget_id(node))
        .expect("the node recorded a body");
    assert!(
        !body.contains(SCRIBBLE_TO),
        "the scribble must start out clear of the node, else the move proves nothing"
    );

    // Right-drag over empty canvas: the gesture latches and paints a polyline
    // nowhere near the node, so nothing is marked.
    harness.press_button_at(PointerButton::Right, SCRIBBLE_FROM);
    harness.drag_to(SCRIBBLE_TO);
    let scribbling = harness.frame_value(|ui| draw(ui, &mut graph_ui, &doc));
    assert!(
        scribbling.is_empty(),
        "a scribble in flight severs nothing until release: {scribbling:?}"
    );

    // Now move the node onto the scribble, centred on its far end. This frame
    // the document says the node is here while its arranged rect still says it
    // is back there — the divergence the probe has to resolve the new way.
    doc.main_view.item_placements[&node] = SCRIBBLE_TO - Vec2::new(body.size.w, body.size.h) * 0.5;
    harness.frame(|ui| {
        draw(ui, &mut graph_ui, &doc);
    });

    harness.release_button(PointerButton::Right);
    let released = harness.frame_value(|ui| draw(ui, &mut graph_ui, &doc));
    // The helper carries the pane assertion: a cut commits against the pane
    // the scribble ran on.
    let released = graph_intents(&released);
    assert!(
        matches!(released[..], [GraphIntent::RemoveNode { node_id }] if *node_id == node),
        "the release cuts the node the scribble now crosses: {released:?}"
    );
}

use super::*;

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
