use glam::Vec2;

use super::*;
use crate::core::document::internals::DocumentFixture;
use crate::gui::pane::graph_ui::internals::CanvasHarness;
use crate::gui::pane::graph_ui::wid;

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

    let mut h = CanvasHarness::new(DocumentFixture::probes(1));
    let node = h.node(0);
    h.prime(2);

    let body = h.node_rect(node);
    assert!(
        !body.contains(SCRIBBLE_TO),
        "the scribble must start out clear of the node, else the move proves nothing"
    );

    // Right-drag over empty canvas: the gesture latches and paints a polyline
    // nowhere near the node, so nothing is marked.
    h.ui.press(SCRIBBLE_FROM, PointerButton::Right);
    h.ui.drag_to(SCRIBBLE_TO);
    let scribbling = h.frame();
    assert!(
        scribbling.is_empty(),
        "a scribble in flight severs nothing until release: {scribbling:?}"
    );

    // Now move the node onto the scribble, centered on its far end. This frame
    // the document says the node is here while its arranged rect still says it
    // is back there — the divergence the probe has to resolve the new way.
    h.document_mut()
        .main_view
        .node_placements
        .get_mut(&node)
        .unwrap()
        .pos = SCRIBBLE_TO - Vec2::new(body.size.w, body.size.h) * 0.5;
    h.frame();

    h.ui.release(PointerButton::Right);
    let released = h.frame();
    assert!(
        matches!(released[..], [GraphIntent::RemoveNode { node_id }] if node_id == node),
        "the release cuts the node the scribble now crosses: {released:?}"
    );
}

/// A node the scribble crossed stays cut when the canvas pans it out of view before the release:
/// a culled node is unpainted, not unprobed.
#[test]
fn the_breaker_cuts_a_node_it_crossed_after_a_pan_moves_it_out_of_view() {
    use palantir::PointerButton;

    let mut h = CanvasHarness::new(DocumentFixture::probes(1));
    let node = h.node(0);
    h.prime(2);
    let body = h.node_rect(node);

    // A stroke across the node's middle, left of it to right of it: it latches where the drag
    // starts, and each later frame extends it to the pointer.
    let y = body.center().y;
    let start = Vec2::new(body.min.x - 40.0, y);
    h.ui.press(start, PointerButton::Right);
    h.ui.drag_to(start + Vec2::new(10.0, 0.0));
    assert!(h.frame().is_empty(), "nothing is cut before the release");
    h.ui.drag_to(Vec2::new(body.min.x + body.size.w + 40.0, y));
    assert!(h.frame().is_empty(), "nothing is cut before the release");

    // Pan the canvas far enough that the node and the stroke's segment over it leave the view.
    h.document_mut().main_view.viewport.pan += Vec2::new(-5000.0, 0.0);
    h.frame();
    assert!(
        h.ui.rect(wid::body(node)).is_none(),
        "the node is culled now"
    );

    h.ui.release(PointerButton::Right);
    let released = h.frame();
    assert!(
        matches!(released[..], [GraphIntent::RemoveNode { node_id }] if node_id == node),
        "the release cuts the node it crossed: {released:?}"
    );
}

/// A wire the scribble crossed stays cut when the canvas pans it out of view before the release,
/// as a node does.
#[test]
fn the_breaker_cuts_a_wire_it_crossed_after_a_pan_moves_it_out_of_view() {
    use palantir::PointerButton;
    use scenarium::{Binding, InputPort};

    use crate::gui::port_ref::PortRef;

    let mut fixture = DocumentFixture::probes(2);
    let (producer, consumer) = (fixture.node(0), fixture.node(1));
    let input = InputPort::new(consumer, 0);
    fixture
        .document
        .graph
        .set_input_binding(input, Binding::wire(producer, 0));
    // Far apart, so the stroke down the wire's middle starts and ends on bare canvas.
    fixture
        .document
        .main_view
        .node_placements
        .get_mut(&consumer)
        .unwrap()
        .pos
        .x += 400.0;
    let mut h = CanvasHarness::new(fixture);
    h.prime(2);

    // Down past the wire's middle, where a cubic with mirrored handles passes; 3 px off it, as
    // the middle is one of the curve's samples, where the crossing test reads touching as missing.
    let middle = (h.port_center(PortRef::output(producer, 0))
        + h.port_center(PortRef::input(consumer, 0)))
        * 0.5
        + Vec2::new(3.0, 0.0);
    let start = middle - Vec2::new(0.0, 40.0);
    h.ui.press(start, PointerButton::Right);
    h.frame();
    h.ui.drag_to(start + Vec2::new(0.0, 6.0));
    h.frame();
    h.ui.drag_to(middle + Vec2::new(0.0, 40.0));
    h.frame();
    h.frame();

    h.document_mut().main_view.viewport.pan += Vec2::new(0.0, -5000.0);
    h.frame();
    h.ui.release(PointerButton::Right);
    let released = h.frame();
    assert!(
        matches!(released[..], [GraphIntent::SetInput { input: cut, to: None }] if cut == input),
        "the release cuts the wire it crossed: {released:?}"
    );
}

/// A release that cuts a node drops the cuts of its own inputs: the removal takes its wires, so
/// a separate unbind of one would only add history. A stroke down across the wire into the
/// consumer's body cuts the consumer alone.
#[test]
fn a_cut_node_takes_the_cuts_of_its_own_inputs() {
    use palantir::PointerButton;
    use scenarium::{Binding, InputPort};

    use crate::gui::port_ref::PortRef;

    let mut fixture = DocumentFixture::probes(2);
    let (producer, consumer) = (fixture.node(0), fixture.node(1));
    fixture
        .document
        .graph
        .set_input_binding(InputPort::new(consumer, 0), Binding::wire(producer, 0));
    fixture
        .document
        .main_view
        .node_placements
        .get_mut(&consumer)
        .unwrap()
        .pos
        .x += 400.0;
    let mut h = CanvasHarness::new(fixture);
    h.prime(2);

    // 3 px off the wire's middle, which is one of its samples: see the wire test above.
    let middle = (h.port_center(PortRef::output(producer, 0))
        + h.port_center(PortRef::input(consumer, 0)))
        * 0.5
        + Vec2::new(3.0, 0.0);
    let start = middle - Vec2::new(0.0, 40.0);
    h.ui.press(start, PointerButton::Right);
    h.frame();
    for to in [
        start + Vec2::new(0.0, 6.0),
        middle + Vec2::new(0.0, 40.0),
        h.node_center(consumer),
    ] {
        h.ui.drag_to(to);
        h.frame();
    }
    h.frame();
    h.ui.release(PointerButton::Right);
    let released = h.frame();
    assert!(
        matches!(released[..], [GraphIntent::RemoveNode { node_id }] if node_id == consumer),
        "the node alone, its input's cut dropped: {released:?}"
    );
}
