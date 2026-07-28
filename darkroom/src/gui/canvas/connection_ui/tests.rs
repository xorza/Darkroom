use palantir::internals::UiHarness;
use scenarium::testing::{TestFuncHooks, test_func_lib};
use scenarium::{Binding, DataType, GraphDef, GraphId, InputPort, Node, NodeId, NodeKind};

use crate::core::document::{BoundarySide, GraphRef, GraphView, PortKind, PortRef};
use crate::core::edit::intent::sink::{Intents, Queued};
use crate::core::edit::intent::types::Intent;
use crate::gui::canvas::connection_ui::{
    ConnectionUI, DragMode, InFlight, commit_connection, fresh_port_name, taken_suffixes,
};
use crate::gui::canvas::geometry::CanvasGeometry;
use crate::gui::canvas::wire::GlyphDrag;
use crate::gui::run_state::RunState;
use crate::gui::scene::{GraphProjection, GraphScene, Scene, SceneSource};

#[derive(Debug)]
struct Fixture {
    scene: Scene,
    target: GraphRef,
    boundary_in: NodeId,
    boundary_out: NodeId,
    mult: NodeId,
}

impl Fixture {
    /// The fixture's sole projected pane.
    fn graph(&self) -> GraphScene<'_> {
        self.scene
            .graph(self.target)
            .expect("fixture pane projected")
    }
}

/// Interior scene of a graph with authored inputs `[input0]` (wired to
/// mult.A) and no outputs: the `GraphInput` node shows ports
/// `[input0, +]`, the `GraphOutput` node just `[+]`.
fn fixture() -> Fixture {
    let library = test_func_lib(TestFuncHooks::default());
    let mult_id = library.by_name("mult").unwrap().id;
    let mut graph =
        GraphDef::new("S").input(scenarium::FuncInput::optional("input0", DataType::Int));
    let boundary_in = graph.body.add(Node::new(NodeKind::GraphInput));
    let boundary_out = graph.body.add(Node::new(NodeKind::GraphOutput));
    let mult = graph.body.add(Node::new(NodeKind::Func(mult_id)));
    graph
        .body
        .set_input_binding(InputPort::new(mult, 0), Binding::bind(boundary_in, 0));

    let view = GraphView::for_graph(&graph.body);
    let def_id = GraphId::unique();
    let mut scene = Scene::default();
    let mut arena = UiHarness::arena();
    scene.rebuild(
        arena.ui(),
        &library,
        &RunState::default(),
        [GraphProjection {
            target: GraphRef::Local(def_id),
            source: SceneSource::Def(&graph),
            view: &view,
        }],
    );
    Fixture {
        scene,
        target: GraphRef::Local(def_id),
        boundary_in,
        boundary_out,
        mult,
    }
}

fn port(node_id: NodeId, kind: PortKind, port_idx: usize) -> PortRef {
    PortRef {
        node_id,
        kind,
        port_idx,
    }
}

/// Commit `start` → `end` on the fixture's pane and return the intents it
/// queued, checking they all landed on that pane's target.
fn committed(fixture: &Fixture, start: PortRef, end: PortRef) -> Vec<Intent> {
    let mut out = Intents::default();
    commit_connection(fixture.graph(), start, end, &mut out);
    out.drain()
        .map(|queued| match queued {
            Queued::Scoped { target, intent } => {
                assert_eq!(
                    target, fixture.target,
                    "a wire commits against its own pane"
                );
                intent
            }
            Queued::Global(intent) => panic!("a wire raises nothing global: {intent:?}"),
        })
        .collect()
}

#[test]
fn fresh_port_name_takes_the_lowest_free_suffix() {
    // Nothing taken: the first slot.
    assert_eq!(fresh_port_name("input", vec![]), "input0");
    // A run from zero fills forward.
    assert_eq!(fresh_port_name("input", vec![0]), "input1");
    assert_eq!(fresh_port_name("output", vec![0, 1, 2]), "output3");
    // A gap is reused rather than skipped, whatever order it arrives in —
    // 1 is free even though 2 and 5 are taken.
    assert_eq!(fresh_port_name("input", vec![0, 2, 5]), "input1");
    assert_eq!(fresh_port_name("input", vec![5, 2, 0]), "input1");
    // Nothing at zero: the whole run sits above the answer.
    assert_eq!(fresh_port_name("input", vec![3, 4]), "input0");
    // Duplicates are stepped past, not counted twice: 0 and 1 are taken, so
    // the answer is 2, not 3.
    assert_eq!(fresh_port_name("input", vec![0, 0, 1]), "input2");
}

#[test]
fn taken_suffixes_reads_only_conforming_names_and_only_at_the_trailing_slot() {
    let mut arena = UiHarness::arena();
    let ui = arena.ui();
    // A column of four: two generated names, one a user rename, one the
    // trailing "+" placeholder itself (whose own name never matters).
    let names = [
        ui.intern("input0"),
        ui.intern("brightness"),
        ui.intern("input3"),
        ui.intern("+"),
    ];

    // Index 3 is the trailing slot, so the three before it are read: the two
    // conforming names contribute 0 and 3; "brightness" can't collide with a
    // generated name, so it contributes nothing.
    let taken = taken_suffixes(names.iter(), 3, "input").expect("index 3 is the trailing slot");
    assert_eq!(taken, vec![0, 3]);
    // Which leaves 1 as the lowest free slot — the gap, not `input4`.
    assert_eq!(fresh_port_name("input", taken), "input1");

    // A different prefix matches none of them, so every slot is free.
    assert_eq!(
        taken_suffixes(names.iter(), 3, "output").expect("still the trailing slot"),
        Vec::<usize>::new()
    );

    // Any index but the last is an existing interface port, not a
    // placeholder — no name is minted for it at all.
    for idx in 0..3 {
        assert_eq!(
            taken_suffixes(names.iter(), idx, "input"),
            None,
            "index {idx} is a real port, not the trailing placeholder"
        );
    }
}

#[test]
#[should_panic(expected = "a wire committed a")]
fn committing_a_same_kind_pair_is_a_broken_invariant_not_a_silent_drop() {
    // `scan_snap_target` only ever offers `start.kind.opposite()`, so a
    // same-kind pair reaching the commit means that broke upstream. Dropping
    // it silently would show up as a wire that simply refuses to land, with
    // nothing anywhere saying why.
    let fixture = fixture();
    let mut out = Intents::default();
    commit_connection(
        fixture.graph(),
        port(fixture.mult, PortKind::Input, 0),
        port(fixture.boundary_in, PortKind::Input, 0),
        &mut out,
    );
}

#[test]
fn wiring_a_placeholder_adds_the_interface_port_before_the_binding() {
    let fixture = fixture();

    // GraphInput placeholder (output idx 1, past authored input0) →
    // mult.B: materialize a fresh input named past the taken "input0",
    // typed from the consumer, then bind.
    let out = committed(
        &fixture,
        port(fixture.boundary_in, PortKind::Output, 1),
        port(fixture.mult, PortKind::Input, 1),
    );
    assert_eq!(out.len(), 2, "add + bind, one batch");
    match &out[0] {
        Intent::AddBoundaryPort {
            side,
            name,
            data_type,
        } => {
            assert_eq!(*side, BoundarySide::Input);
            assert_eq!(name, "input1", "\"input0\" is taken");
            assert_eq!(*data_type, DataType::Int, "typed from mult.B");
        }
        other => panic!("expected AddBoundaryPort, got {other:?}"),
    }
    match &out[1] {
        Intent::SetInput { input, to } => {
            assert_eq!(*input, InputPort::new(fixture.mult, 1));
            assert_eq!(*to, Some(Binding::bind(fixture.boundary_in, 1)));
        }
        other => panic!("expected SetInput, got {other:?}"),
    }

    // mult.Prod → GraphOutput placeholder (input idx 0): symmetric,
    // typed from the producer's resolved output.
    let out = committed(
        &fixture,
        port(fixture.mult, PortKind::Output, 0),
        port(fixture.boundary_out, PortKind::Input, 0),
    );
    assert_eq!(out.len(), 2);
    match &out[0] {
        Intent::AddBoundaryPort {
            side,
            name,
            data_type,
        } => {
            assert_eq!(*side, BoundarySide::Output);
            assert_eq!(name, "output0");
            assert_eq!(*data_type, DataType::Int, "typed from mult.Prod");
        }
        other => panic!("expected AddBoundaryPort, got {other:?}"),
    }

    // An existing interface port (input0, idx 0) is not a placeholder:
    // rewiring it emits only the binding.
    let out = committed(
        &fixture,
        port(fixture.boundary_in, PortKind::Output, 0),
        port(fixture.mult, PortKind::Input, 1),
    );
    assert_eq!(out.len(), 1, "no interface change for an existing port");
    assert!(matches!(&out[0], Intent::SetInput { .. }));
}

/// Drive one prepass with a wire already in flight from `start`,
/// returning the gesture state that survived it. `Floating` because it
/// is the mode that survives a quiet frame (a `Held` wire reads the
/// release edge off the port geometry, which a bare fixture has none
/// of) — and the more exposed one: no button is held, so it can sit
/// across an arbitrary number of undos.
fn prepass_with_wire_from(scene: &Scene, start: PortRef) -> Option<InFlight> {
    let mut arena = UiHarness::arena();
    // The fixture is one pane, and it isn't `Main` — latch on whichever
    // it built, the way the production latch resolves it from the node.
    let target = scene.graphs().next().expect("fixture has a pane").target();
    let mut connections = ConnectionUI::default();
    connections.state.latch(
        target,
        InFlight {
            drag: GlyphDrag::new(start),
            mode: DragMode::Floating,
        },
    );
    let mut out = Intents::default();
    connections.apply(
        arena.ui(),
        scene,
        &CanvasGeometry::default(),
        None,
        false,
        &mut out,
    );
    assert!(out.is_empty(), "an untouched prepass emits nothing");
    connections.state.get(target).copied()
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
    let live = PortRef {
        node_id: f.mult,
        kind: PortKind::Output,
        port_idx: 0,
    };
    assert!(
        prepass_with_wire_from(&f.scene, live).is_some(),
        "a wire from a node still in the scene stays in flight"
    );

    let gone = PortRef {
        node_id: NodeId::unique(),
        ..live
    };
    assert!(
        prepass_with_wire_from(&f.scene, gone).is_none(),
        "a wire from a vanished node drops"
    );
}
