use glam::{UVec2, Vec2};
use palantir::Harness;
use scenarium::internals::test_graph::NodeSpec;
use scenarium::internals::test_graph::TestGraph;
use scenarium::{DataType, InputPort, NodeId};

use super::*;
use crate::core::document::internals::DocumentFixture;
use crate::gui::frame::requests::Requests;
use crate::gui::graph_ctx::internals::GraphCtxFixture;
use crate::gui::pane::graph_ui::canvas_input::CanvasInput;
use crate::gui::pane::graph_ui::internals::CanvasHarness;
use crate::gui::pane::graph_ui::unsettled_ctx::UnsettledCtx;
use crate::gui::pane::graph_ui::wid;

/// Two two-in/one-out nodes wired producer → consumer: enough graph for a wire to be in flight
/// over, and for the snap filter to have a cycle question to answer. Both share one
/// declaration, so a port index means the same thing on either end.
#[derive(Debug)]
struct Wired {
    fixture: GraphCtxFixture,
    producer: NodeId,
    consumer: NodeId,
}

fn fixture() -> Wired {
    let mut g = TestGraph::new();
    g.add("producer", NodeSpec::mult);
    g.instance("consumer", "producer");
    g.wire("producer", "consumer");
    let (producer, consumer) = (g.id("producer"), g.id("consumer"));

    Wired {
        fixture: GraphCtxFixture::over(g),
        producer,
        consumer,
    }
}

#[test]
#[should_panic(expected = "a wire committed a")]
fn committing_a_same_kind_pair_is_a_broken_invariant_not_a_silent_drop() {
    // `scan_snap_target` only offers `start.kind.opposite()`, so a same-kind pair reaching the
    // commit means that broke upstream; dropping it would show as a wire that refuses to land.
    let Wired {
        producer, consumer, ..
    } = fixture();
    wire(PortRef::input(consumer, 0), PortRef::input(producer, 0));
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
    let output = PortRef::output(producer, 0);

    for (input, accepted) in [
        (PortRef::input(any, 0), true),
        (PortRef::input(text, 0), false),
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

/// Run one canvas input stage with a wire latched from `start` and report whether it is still
/// in flight afterwards. Deliberately `Floating`: the mode the fixture can express (a `Held`
/// wire needs a real button release edge off the port geometry), and the more exposed one, as
/// it can sit across any number of undos.
fn input_with_wire_from(fixture: &GraphCtxFixture, start: PortRef) -> Option<InFlight> {
    let mut arena = Harness::builder(UVec2::ONE).mono().build();
    let mut wires = WireUi {
        state: Some(InFlight {
            drag: GlyphDrag::new(start),
            mode: DragMode::Floating,
        }),
    };
    let mut out = Requests::default();
    // The canvas state a context carries beside the pane, empty: a fixture records nothing, so
    // there are no port centers. No gesture latched and no Esc either, as these tests drive the
    // wire directly.
    let geometry = CanvasGeometry::default();
    let ctx = CanvasCtx::new(
        UnsettledCtx::new(fixture.graph_ctx(), CanvasInput::default()),
        &geometry,
    );
    wires.input(arena.ui_mut(), &mut Stage { ctx, out: &mut out });
    assert!(
        out.document().is_empty(),
        "an untouched input stage emits nothing"
    );
    wires.state.as_ref().copied()
}

#[test]
fn a_wire_drops_when_its_start_node_leaves_the_scene() {
    // Undo runs before the canvas input stage, so the node a wire grew out of can vanish
    // mid-drag. The gesture has to let go: until then `GraphCtx::port_type` reports the start
    // as untyped, which `scan_snap_target` reads as "compatible with anything", so a stranded
    // wire would snap onto ports it should never accept.
    let Wired {
        fixture: f,
        producer,
        ..
    } = fixture();
    let live = PortRef::output(producer, 0);
    assert!(
        input_with_wire_from(&f, live).is_some(),
        "a wire from a node still in the scene stays in flight"
    );

    let gone = PortRef::output(NodeId::unique(), 0);
    assert!(
        input_with_wire_from(&f, gone).is_none(),
        "a wire from a vanished node drops"
    );
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

    let source = wid::port_circle(PortRef::output(producer, 0));
    let drop_at = h.port_center(PortRef::input(consumer, 0));

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
                if input == InputPort::new(consumer, 0)
                    && src.node_id == producer
                    && src.port_idx == 0
        ),
        "the release binds the port it snapped to, and nothing else: {released:?}"
    );

    // The same gesture backwards, with the document holding the edge that release just
    // described: consumer.out0 → producer.in0 would close the loop, so the wire must not snap
    // and the release must commit nothing. The filter answers that off `Document::graph_for`.
    h.document_mut()
        .graph
        .set_input_binding(InputPort::new(consumer, 0), Binding::wire(producer, 0));
    let back_source = wid::port_circle(PortRef::output(consumer, 0));
    let back_drop_at = h.port_center(PortRef::input(producer, 0));
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

/// A right press that ends a floating wire spends its click: the release, a frame later, opens no
/// palette, where a right click alone on bare canvas opens one.
#[test]
fn a_right_press_that_ends_a_floating_wire_opens_no_palette() {
    use palantir::PointerButton;

    let mut h = CanvasHarness::new(DocumentFixture::probes(2));
    let start = PortRef::output(h.node(0), 0);
    h.prime(2);
    let bare = Vec2::new(600.0, 600.0);
    let right_click = |h: &mut CanvasHarness| {
        h.ui.press(bare, PointerButton::Right);
        h.frame();
        h.ui.release(PointerButton::Right);
        h.frame();
    };

    let CanvasHarness { graph_ui, ctx, .. } = &mut h;
    graph_ui.gestures.wire_ui.resume(start, ctx.graph_ctx());
    // A frame for the floating wire to ask for the pointer's presses.
    h.frame();
    assert!(h.graph_ui.gestures.wire_ui.in_flight());
    right_click(&mut h);
    assert!(
        !h.graph_ui.gestures.wire_ui.in_flight(),
        "the press ended the wire"
    );
    assert!(
        !h.graph_ui.gestures.new_node_ui.in_flight(),
        "its release opened no palette"
    );

    h.advance_past_double_click();
    right_click(&mut h);
    assert!(
        h.graph_ui.gestures.new_node_ui.in_flight(),
        "a right click alone opens it"
    );
}
