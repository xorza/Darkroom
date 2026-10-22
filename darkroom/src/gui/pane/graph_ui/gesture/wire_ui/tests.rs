use palantir::internals::ui_harness::UiHarness;
use scenarium::internals::test_graph::NodeSpec;
use scenarium::internals::test_graph::TestGraph;
use scenarium::{DataType, InputPort, NodeId};

use super::*;
use crate::core::document::internals::DocumentFixture;
use crate::gui::graph_ctx::internals::GraphCtxFixture;
use crate::gui::pane::graph_ui::internals::CanvasHarness;
use crate::gui::pane::graph_ui::node_ui::port_row;
use crate::gui::pane::graph_ui::prepass_ctx::{CanvasInput, PrepassCtx};

/// Two two-in/one-out nodes wired producer → consumer — enough graph for a
/// wire to be in flight over, and enough wiring for the snap filter to have a
/// cycle question to answer. Both share one declaration, so a port index means
/// the same thing on either end.
///
/// Returned beside the fixture rather than looked up out of it: the snap
/// filter reads the authoring graph to answer its cycle question, so a test
/// needs to name both ends.
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
    // `scan_snap_target` only ever offers `start.kind.opposite()`, so a
    // same-kind pair reaching the commit means that broke upstream. Dropping
    // it silently would show up as a wire that simply refuses to land, with
    // nothing anywhere saying why.
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

/// Run one canvas prepass with a wire latched from `start` and report whether
/// it is still in flight afterwards. Deliberately `Floating`: it is the mode
/// the fixture can express (a `Held` wire needs a real button release edge off
/// the port geometry, which a bare fixture has none of) — and the more exposed
/// one, since no button is held, so it can sit across an arbitrary number of
/// undos.
fn prepass_with_wire_from(fixture: &GraphCtxFixture, start: PortRef) -> Option<InFlight> {
    let mut arena = UiHarness::arena();
    let mut wires = WireUi::default();
    wires.state.latch(InFlight {
        drag: GlyphDrag::new(start),
        mode: DragMode::Floating,
    });
    let mut out = Requests::default();
    // The canvas state a context carries beside the pane, empty: a fixture
    // records nothing, so there are no port centers to cache. No gesture
    // latched and no Esc either — these tests drive the wire directly rather
    // than through the bare-canvas classification.
    let geometry = CanvasGeometry::default();
    let ctx = CanvasCtx::new(
        PrepassCtx::new(fixture.graph_ctx(), CanvasInput::default()),
        &geometry,
    );
    wires.apply(arena.ui(), ctx, &mut out);
    assert!(
        out.document().is_empty(),
        "an untouched prepass emits nothing"
    );
    wires.state.get().copied()
}

#[test]
fn a_wire_drops_when_its_start_node_leaves_the_scene() {
    // Undo runs before the canvas prepass, so the node a wire grew out
    // of can vanish mid-drag. The gesture has to let go: a commit
    // against a dead producer is refused at the edit boundary anyway
    // (silently), and until then `GraphCtx::port_type` reports the start as
    // untyped, which `scan_snap_target` reads as "compatible with
    // anything" — so a stranded wire would snap onto ports it should
    // never accept.
    let Wired {
        fixture: f,
        producer,
        ..
    } = fixture();
    let live = PortRef::output(producer, 0);
    assert!(
        prepass_with_wire_from(&f, live).is_some(),
        "a wire from a node still in the scene stays in flight"
    );

    let gone = PortRef::output(NodeId::unique(), 0);
    assert!(
        prepass_with_wire_from(&f, gone).is_none(),
        "a wire from a vanished node drops"
    );
}

/// A bare drag off an output port onto a compatible input commits the bind.
///
/// Drives the whole `GlyphDrag` lifecycle through real input rather than
/// poking the controller: the latch off the port layer's drag edge, the
/// per-frame snap scan (which is a geometry hit test precisely *because*
/// palantir hides `hovered` from every widget but the drag-capture owner), and
/// the release edge — the layer's `dragging` flag going false, which is the
/// only thing that says a held wire is done.
#[test]
fn a_port_drag_released_over_a_compatible_port_commits_the_binding() {
    use palantir::PointerButton;

    // Two unwired nodes, so the drag has a producer to leave and a consumer
    // to land on and nothing to trip the cycle check.
    let mut h = CanvasHarness::new(DocumentFixture::probes(2));
    let (producer, consumer) = (h.node(0), h.node(1));
    // Two frames to record both nodes, so their port circles have widget ids
    // and `CanvasGeometry` measured centers to hit-test against.
    h.prime(2);

    let source = port_row::port_circle_wid(PortRef::output(producer, 0));
    let drop_at = h.port_center(PortRef::input(consumer, 0));

    h.ui.press_on(source);
    h.frame();
    h.ui.drag_to(drop_at);
    let held = h.frame();
    assert!(
        held.is_empty(),
        "a wire still held commits nothing: {held:?}"
    );

    h.ui.release_button(PointerButton::Left);
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

    // Now the same gesture backwards, with the document holding the edge that
    // release just described: consumer.out0 → producer.in0 would close the
    // loop, so the wire must not snap and the release must commit nothing.
    // The filter answers that off `Document::graph_for` — a prepass handed the
    // wrong graph would go on snapping cycles with every other assertion here
    // still green.
    h.document_mut()
        .graph
        .set_input_binding(InputPort::new(consumer, 0), Binding::wire(producer, 0));
    let back_source = port_row::port_circle_wid(PortRef::output(consumer, 0));
    let back_drop_at = h.port_center(PortRef::input(producer, 0));
    h.advance_past_double_click();
    h.ui.press_on(back_source);
    h.frame();
    h.ui.drag_to(back_drop_at);
    h.frame();
    h.ui.release_button(PointerButton::Left);
    let refused = h.frame();
    assert!(
        refused.is_empty(),
        "a drop that would close a cycle never snaps, so it binds nothing: {refused:?}"
    );
}
