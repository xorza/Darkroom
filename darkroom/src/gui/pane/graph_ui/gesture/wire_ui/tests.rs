use scenarium::internals::test_graph::TestGraph;
use scenarium::{DataType, NodeId};

use super::*;
use crate::core::document::internals::DocumentFixture;
use crate::gui::graph_ctx::internals::GraphCtxFixture;
use crate::gui::pane::graph_ui::internals::CanvasHarness;
use crate::gui::pane::graph_ui::wid;

#[test]
#[should_panic(expected = "a wire committed a")]
fn committing_a_same_kind_pair_is_a_broken_invariant_not_a_silent_drop() {
    // `scan_snap_target` only offers `start.kind.opposite()`, so a same-kind pair reaching the
    // commit means that broke upstream; dropping it would show as a wire that refuses to land.
    let (producer, consumer) = (NodeId::from_u128(1), NodeId::from_u128(2));
    wire(
        PortRef::input(consumer, PortId::new(1)),
        PortRef::input(producer, PortId::new(1)),
    );
}

/// The snap filter reads the input's type against the output's, whichever end
/// the drag started from: an `Any` input takes an `Int` output, and a `String`
/// input does not.
#[test]
fn the_snap_type_check_reads_the_input_against_the_output_from_either_end() {
    let mut g = TestGraph::new();
    let producer = g.add("producer", |n| n.output(DataType::Int));
    let any = g.add("any", |n| n.input(DataType::Any));
    let text = g.add("text", |n| n.input(DataType::String));
    let f = GraphCtxFixture::over(g);
    let graph_ctx = f.graph_ctx();
    let output = PortRef::output(producer, PortId::new(1));

    for (input, accepted) in [
        (PortRef::input(any, PortId::new(1)), true),
        (PortRef::input(text, PortId::new(1)), false),
    ] {
        assert_eq!(
            accepts_wire(graph_ctx, output, input),
            accepted,
            "{input:?} from the output"
        );
        assert_eq!(
            accepts_wire(graph_ctx, input, output),
            accepted,
            "{input:?} from the input"
        );
    }
}

/// Undo runs before the canvas input stage, so the node a wire grew out of can vanish
/// mid-drag. The gesture has to let go: until then `GraphCtx::port_type` reports the start as
/// untyped, which `scan_snap_target` reads as "compatible with anything", so a stranded wire
/// would snap onto ports it should never accept. Held over the other node's input, its release
/// then binds nothing.
#[test]
fn a_wire_drops_when_its_start_node_leaves_the_scene() {
    use palantir::PointerButton;

    let mut h = CanvasHarness::new(DocumentFixture::probes(2));
    let (producer, consumer) = (h.node(0), h.node(1));
    h.prime(2);
    let source = wid::port_circle(GraphId::Main, PortRef::output(producer, PortId::new(1)));
    let over = h.port_center(PortRef::input(consumer, PortId::new(1)));
    h.ui.press(source, PointerButton::Left);
    h.frame();
    h.ui.drag_to(over);
    h.frame();
    assert!(h.graph_ui.gestures.wire_ui.in_flight(), "the wire is held");

    let document = h.document_mut();
    document.graphs.main_mut().detach_node(producer);
    document.main_view.node_placements.remove(&producer);
    h.frame();
    assert!(
        !h.graph_ui.gestures.wire_ui.in_flight(),
        "a wire from a vanished node drops"
    );
    h.ui.release(PointerButton::Left);
    assert!(h.frame().is_empty(), "and its release binds nothing");
}

/// A bare drag off an output port onto a compatible input commits the bind.
///
/// Drives the whole `GlyphDrag` lifecycle through real input: the latch off the port layer's
/// drag edge, the per-frame snap scan (a geometry hit test, as palantir hides `hovered` from
/// every widget but the drag-capture owner), and the release edge, the layer's `dragging` flag
/// going false.
#[test]
fn a_port_drag_released_over_a_compatible_port_commits_the_binding() {
    use palantir::PointerButton;

    // Two unwired nodes: a producer to leave, a consumer to land on, nothing to trip the cycle
    // check.
    let mut h = CanvasHarness::new(DocumentFixture::probes(2));
    let (producer, consumer) = (h.node(0), h.node(1));
    // Two frames, so the port circles have widget ids and `CanvasGeometry` measured centers.
    h.prime(2);

    let source = wid::port_circle(GraphId::Main, PortRef::output(producer, PortId::new(1)));
    let drop_at = h.port_center(PortRef::input(consumer, PortId::new(1)));

    h.ui.press(source, PointerButton::Left);
    h.frame();
    h.ui.drag_to(drop_at);
    let held = h.frame();
    assert!(
        held.is_empty(),
        "a wire still held commits nothing: {held:?}"
    );

    h.ui.release(PointerButton::Left);
    let released = h.frame();
    assert!(
        matches!(
            released[..],
            [GraphIntent::SetInput { input, to: Some(Binding::Wire(src)) }]
                if input == InputPort::new(consumer, PortId::new(1))
                    && src.node_id == producer
                    && src.port == PortId::new(1)
        ),
        "the release binds the port it snapped to, and nothing else: {released:?}"
    );

    // The same gesture backwards, with the document holding the edge that release just
    // described: consumer.out0 → producer.in0 would close the loop, so the wire must not snap
    // and the release must commit nothing. The filter answers that off `Document::graph_for`.
    h.document_mut().graphs.main_mut().set_input_binding(
        InputPort::new(consumer, PortId::new(1)),
        Binding::wire(producer, PortId::new(1)),
    );
    let back_source = wid::port_circle(GraphId::Main, PortRef::output(consumer, PortId::new(1)));
    let back_drop_at = h.port_center(PortRef::input(producer, PortId::new(1)));
    h.advance_past_double_click();
    h.ui.press(back_source, PointerButton::Left);
    h.frame();
    h.ui.drag_to(back_drop_at);
    h.frame();
    h.ui.release(PointerButton::Left);
    let refused = h.frame();
    assert!(
        refused.is_empty(),
        "a drop that would close a cycle never snaps, so it binds nothing: {refused:?}"
    );
}
