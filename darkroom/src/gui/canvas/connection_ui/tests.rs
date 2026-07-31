use palantir::internals::UiHarness;
use scenarium::testing::graph::TestGraph;
use scenarium::{Library, NodeId, OutputTypes};

use super::*;
use crate::core::document::Document;
use crate::gui::app::{AppContext, StatusInputs};
use crate::gui::run_state::RunState;

#[derive(Debug)]
struct Fixture {
    /// Everything a scope composes. The snap filter reads the authoring
    /// graph out of the document to answer its cycle question, each node's
    /// ports out of the library, and a port's resolved type off the table —
    /// which composing the scope fills.
    doc: Document,
    library: Library,
    run_state: RunState,
    theme: Theme,
    output_types: OutputTypes,
    producer: NodeId,
    consumer: NodeId,
}

impl Fixture {
    fn graph_scope(&mut self) -> GraphScope<'_> {
        let app = AppContext::new(
            &self.theme,
            &self.library,
            &self.run_state,
            StatusInputs::default(),
        );
        GraphScope::for_document(app, &self.doc, &mut self.output_types)
            .expect("the fixture's document shows the graph")
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
    connections.apply(
        arena.ui(),
        fixture.graph_scope(),
        &CanvasGeometry::default(),
        None,
        false,
        &mut out,
    );
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
