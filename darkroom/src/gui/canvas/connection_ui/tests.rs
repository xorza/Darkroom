use palantir::internals::UiHarness;
use scenarium::testing::{TestFuncHooks, test_func_lib};
use scenarium::{Binding, InputPort, Node, NodeId, NodeKind};

use super::*;
use crate::core::document::Document;
use crate::gui::run_state::RunState;
use crate::gui::scene::Scene;

#[derive(Debug)]
struct Fixture {
    scene: Scene,
    /// The document the scene was projected from — the prepass resolves the
    /// pane's authoring graph out of it to answer the snap filter's cycle
    /// question.
    doc: Document,
    producer: NodeId,
    consumer: NodeId,
}

impl Fixture {
    fn frame(&self) -> Frame<'_> {
        Frame {
            scene: &self.scene,
            doc: &self.doc,
        }
    }
}

/// Two `mult` nodes wired producer → consumer, projected — enough scene for a
/// wire to be in flight over, and enough wiring for the snap filter to have a
/// cycle question to answer.
fn fixture() -> Fixture {
    let library = test_func_lib(TestFuncHooks::default());
    let mult_id = library.by_name("mult").unwrap().id;
    let mut graph = scenarium::Graph::default();
    let producer = graph.add(Node::new(NodeKind::Func(mult_id)));
    let consumer = graph.add(Node::new(NodeKind::Func(mult_id)));
    graph.set_input_binding(InputPort::new(consumer, 0), Binding::bind(producer, 0));

    let doc = Document::from(graph);
    let mut scene = Scene::default();
    let mut arena = UiHarness::arena();
    scene.rebuild(arena.ui(), &library, &RunState::default(), &doc);
    Fixture {
        scene,
        doc,
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
fn prepass_with_wire_from(fixture: &Fixture, start: PortRef) -> Option<InFlight> {
    let mut arena = UiHarness::arena();
    let mut connections = ConnectionUI::default();
    connections.state.latch(InFlight {
        drag: GlyphDrag::new(start),
        mode: DragMode::Floating,
    });
    let mut out = Intents::default();
    connections.apply(
        arena.ui(),
        fixture.frame(),
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
    let f = fixture();
    let live = port(f.producer, PortKind::Output, 0);
    assert!(
        prepass_with_wire_from(&f, live).is_some(),
        "a wire from a node still in the scene stays in flight"
    );

    let gone = PortRef {
        node_id: NodeId::unique(),
        ..live
    };
    assert!(
        prepass_with_wire_from(&f, gone).is_none(),
        "a wire from a vanished node drops"
    );
}
