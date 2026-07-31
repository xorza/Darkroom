use glam::UVec2;
use palantir::internals::UiHarness;
use palantir::{Configure, Panel, Sizing, Ui};
use scenarium::testing::graph::TestGraph;
use scenarium::{Library, NodeId, OutputTypes};

use super::*;
use crate::core::document::harness::{one_func_library, spread};
use crate::core::document::{Document, GraphView};
use crate::core::edit::intent::sink::Intents;
use crate::gui::app::ctx::{AppCtx, StatusInputs};
use crate::gui::pane::graph::GraphUI;
use crate::gui::pane::graph::frame::hits::CanvasHits;
use crate::gui::pane::graph::harness::*;
use crate::gui::pane::graph::node::port_row::port_circle_wid;
use crate::gui::state::run_state::RunState;
use crate::gui::theme::Theme;

#[derive(Debug)]
struct Fixture {
    /// Everything a context composes. The snap filter reads the authoring
    /// graph out of the document to answer its cycle question, each node's
    /// ports out of the library, and a port's resolved type off the table —
    /// which composing the context fills.
    doc: Document,
    library: Library,
    run_state: RunState,
    theme: Theme,
    output_types: OutputTypes,
    /// The canvas state a context carries beside the pane. Both start
    /// empty: a bare fixture records nothing, so there are no port centers
    /// to cache and no responses to sweep.
    geometry: CanvasGeometry,
    hits: CanvasHits,
    producer: NodeId,
    consumer: NodeId,
}

impl Fixture {
    /// The canvas context a controller is driven through. No gesture latched
    /// and no Esc: these tests drive the wire directly rather than through
    /// the bare-canvas classification.
    fn canvas_ctx(&mut self) -> CanvasCtx<'_> {
        let app = AppCtx::new(
            &self.theme,
            &self.library,
            &self.run_state,
            StatusInputs::default(),
        );
        let graph_ctx = GraphCtx::for_document(app, &self.doc, &mut self.output_types)
            .expect("the fixture's document shows the graph");
        CanvasCtx::new(graph_ctx, &self.geometry, &self.hits, None, false)
    }
}

/// Two two-in/one-out nodes wired producer → consumer — enough graph for a
/// wire to be in flight over, and enough wiring for the snap filter to have a
/// cycle question to answer. Both share one declaration, so a port index means
/// the same thing on either end.
fn fixture() -> Fixture {
    let mut g = TestGraph::new();
    g.add("producer", |n| n.mult());
    g.instance("consumer", "producer");
    g.wire("producer", 0, "consumer", 0);
    let (producer, consumer) = (g.id("producer"), g.id("consumer"));

    Fixture {
        doc: Document::from(g.graph),
        library: g.library,
        run_state: RunState::default(),
        theme: Theme::default(),
        output_types: OutputTypes::default(),
        geometry: CanvasGeometry::default(),
        hits: CanvasHits::default(),
        producer,
        consumer,
    }
}

fn port(node_id: NodeId, kind: PortKind, port_idx: usize) -> PortRef {
    PortRef {
        node_id,
        kind,
        port_idx,
    }
}

#[test]
#[should_panic(expected = "a wire committed a")]
fn committing_a_same_kind_pair_is_a_broken_invariant_not_a_silent_drop() {
    // `scan_snap_target` only ever offers `start.kind.opposite()`, so a
    // same-kind pair reaching the commit means that broke upstream. Dropping
    // it silently would show up as a wire that simply refuses to land, with
    // nothing anywhere saying why.
    let f = fixture();
    let mut out = Intents::default();
    commit_connection(
        port(f.consumer, PortKind::Input, 0),
        port(f.producer, PortKind::Input, 0),
        &mut out,
    );
}

/// Run one canvas prepass with a wire latched from `start` and report whether
/// it is still in flight afterwards. Deliberately `Floating`: it is the mode
/// the fixture can express (a `Held` wire needs a real button release edge off
/// the port geometry, which a bare fixture has none of) — and the more exposed
/// one, since no button is held, so it can sit across an arbitrary number of
/// undos.
fn prepass_with_wire_from(fixture: &mut Fixture, start: PortRef) -> Option<InFlight> {
    let mut arena = UiHarness::arena();
    let mut connections = ConnectionUI::default();
    connections.state.latch(InFlight {
        drag: GlyphDrag::new(start),
        mode: DragMode::Floating,
    });
    let mut out = Intents::default();
    connections.apply(arena.ui(), fixture.canvas_ctx(), None, &mut out);
    assert!(out.is_empty(), "an untouched prepass emits nothing");
    connections.state.get().copied()
}

#[test]
fn a_wire_drops_when_its_start_node_leaves_the_scene() {
    // Undo runs before the canvas prepass, so the node a wire grew out
    // of can vanish mid-drag. The gesture has to let go: a commit
    // against a dead producer is refused at the edit boundary anyway
    // (silently), and until then `port_data_type` reports the start as
    // untyped, which `scan_snap_target` reads as "compatible with
    // anything" — so a stranded wire would snap onto ports it should
    // never accept.
    let mut f = fixture();
    let live = port(f.producer, PortKind::Output, 0);
    assert!(
        prepass_with_wire_from(&mut f, live).is_some(),
        "a wire from a node still in the scene stays in flight"
    );

    let gone = PortRef {
        node_id: NodeId::unique(),
        ..live
    };
    assert!(
        prepass_with_wire_from(&mut f, gone).is_none(),
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

    let library = one_func_library();
    let probe = library.by_name("probe").expect("just added").clone();

    // Two unwired nodes, so the drag has a producer to leave and a consumer
    // to land on and nothing to trip the cycle check.
    let mut doc = Document::default();
    let producer = doc.graph.add_func_node(&probe);
    let consumer = doc.graph.add_func_node(&probe);
    doc.main_view = GraphView::for_graph(&doc.graph);
    spread(&mut doc.main_view);

    let theme = Theme::default();
    let run_state = RunState::default();
    let mut harness = UiHarness::new(UVec2::new(1200, 800));
    let mut graph_ui = GraphUI::default();

    // `doc` is a parameter, not a capture, so the graph can gain the edge the
    // first drag commits before the second one runs against it.
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

    // Two frames to record both nodes, so their port circles have widget ids
    // and `CanvasGeometry` measured centers to hit-test against.
    harness.frame(|ui| {
        draw(ui, &mut graph_ui, &doc);
    });
    harness.frame(|ui| {
        draw(ui, &mut graph_ui, &doc);
    });

    let source = port_circle_wid(PortRef {
        node_id: producer,
        kind: PortKind::Output,
        port_idx: 0,
    });
    let sink = port_circle_wid(PortRef {
        node_id: consumer,
        kind: PortKind::Input,
        port_idx: 0,
    });
    // The snap scan tests the post-transform rect, so aim at that one.
    let drop_at = harness.center_of(sink);

    harness.press_on(source);
    harness.frame(|ui| {
        draw(ui, &mut graph_ui, &doc);
    });
    harness.drag_to(drop_at);
    let held = harness.frame_value(|ui| draw(ui, &mut graph_ui, &doc));
    assert!(
        held.is_empty(),
        "a wire still held commits nothing: {held:?}"
    );

    harness.release_button(PointerButton::Left);
    let released = harness.frame_value(|ui| draw(ui, &mut graph_ui, &doc));

    // The helper carries the pane assertion: a wire commits against the pane
    // holding its start node, never the focused one.
    let released = graph_intents(&released);
    assert!(
        matches!(
            released[..],
            [GraphIntent::SetInput { input, to: Some(Binding::Bind(src)) }]
                if *input == InputPort::new(consumer, 0)
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
    doc.graph
        .set_input_binding(InputPort::new(consumer, 0), Binding::bind(producer, 0));
    let back_source = port_circle_wid(PortRef {
        node_id: consumer,
        kind: PortKind::Output,
        port_idx: 0,
    });
    let back_sink = port_circle_wid(PortRef {
        node_id: producer,
        kind: PortKind::Input,
        port_idx: 0,
    });
    let back_drop_at = harness.center_of(back_sink);
    harness.advance_past_double_click(|ui| {
        draw(ui, &mut graph_ui, &doc);
    });
    harness.press_on(back_source);
    harness.frame(|ui| {
        draw(ui, &mut graph_ui, &doc);
    });
    harness.drag_to(back_drop_at);
    harness.frame(|ui| {
        draw(ui, &mut graph_ui, &doc);
    });
    harness.release_button(PointerButton::Left);
    let refused = harness.frame_value(|ui| draw(ui, &mut graph_ui, &doc));
    assert!(
        refused.is_empty(),
        "a drop that would close a cycle never snaps, so it binds nothing: {refused:?}"
    );
}
