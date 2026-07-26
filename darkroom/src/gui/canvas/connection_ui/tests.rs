use palantir::Ui;
use scenarium::testing::{TestFuncHooks, test_func_lib};
use scenarium::{Binding, DataType, GraphDef, InputPort, Node, NodeId, NodeKind};

use crate::core::document::{BoundarySide, GraphView, PortKind, PortRef};
use crate::core::edit::intent::types::Intent;
use crate::gui::canvas::connection_ui::{ConnectionUI, DragMode, InFlight, commit_connection};
use crate::gui::canvas::geometry::CanvasGeometry;
use crate::gui::run_state::RunState;
use crate::gui::scene::{Scene, SceneSource};

#[derive(Debug)]
struct Fixture {
    scene: Scene,
    boundary_in: NodeId,
    boundary_out: NodeId,
    mult: NodeId,
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
    let mut scene = Scene::default();
    let mut ui = Ui::default();
    scene.rebuild(
        &mut ui,
        SceneSource::Def(&graph),
        &view,
        &library,
        &RunState::default(),
        false,
    );
    Fixture {
        scene,
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

#[test]
fn wiring_a_placeholder_adds_the_interface_port_before_the_binding() {
    let fixture = fixture();

    // GraphInput placeholder (output idx 1, past authored input0) →
    // mult.B: materialize a fresh input named past the taken "input0",
    // typed from the consumer, then bind.
    let mut out = Vec::new();
    commit_connection(
        &fixture.scene,
        port(fixture.boundary_in, PortKind::Output, 1),
        port(fixture.mult, PortKind::Input, 1),
        &mut out,
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
    let mut out = Vec::new();
    commit_connection(
        &fixture.scene,
        port(fixture.mult, PortKind::Output, 0),
        port(fixture.boundary_out, PortKind::Input, 0),
        &mut out,
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
    let mut out = Vec::new();
    commit_connection(
        &fixture.scene,
        port(fixture.boundary_in, PortKind::Output, 0),
        port(fixture.mult, PortKind::Input, 1),
        &mut out,
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
    let mut ui = Ui::default();
    let mut connections = ConnectionUI {
        state: Some(InFlight {
            start,
            snap_end: None,
            mode: DragMode::Floating,
        }),
        ..Default::default()
    };
    let mut out = Vec::new();
    connections.apply(&mut ui, scene, &CanvasGeometry::default(), None, &mut out);
    assert!(out.is_empty(), "an untouched prepass emits nothing");
    connections.state
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
