use std::sync::Arc;

use scenarium::{FileKind, FileKindId, FileMode, GraphId, PortId};

use super::*;

/// The type line of a port tooltip: a path names its kind, mode and extensions, everything else
/// the library's own name for the type, so a tooltip and a palette entry can't disagree.
#[test]
fn type_label_names_a_path_by_its_kind_and_any_other_type_as_the_library_does() {
    let image = FileKindId::from_u128(1);
    let mut library = Library::default();
    library.register_file_kind(image, FileKind::new("Image", ["png"]));
    let graphs = GraphSet::default();
    let label = |data_type| {
        TypeLabel {
            graphs: &graphs,
            library: &library,
            data_type,
        }
        .to_string()
    };
    assert_eq!(label(DataType::Any), "Any");
    assert_eq!(label(DataType::Float), "Float");
    assert_eq!(
        label(DataType::File {
            kind: image,
            mode: FileMode::Save,
        }),
        "Image · save path (png)"
    );
}

/// "Add preview" spawns a node already reading the port it was raised
/// from, offset clear of it, as one batch — so a single undo removes both
/// the node and its wire.
#[test]
fn add_preview_spawns_a_node_already_wired_to_the_port() {
    use scenarium::{InputPort, OutputPort};

    let func = preview_sink::func(Arc::default());
    let producer = NodeId::unique();
    let port = OutputPort::new(producer, PortId::new(3));
    let center = Vec2::new(100.0, 40.0);

    let node_id = NodeId::unique();
    let [add, wire] = PreviewSpawn {
        port,
        pos: center + PREVIEW_SPAWN_OFFSET,
        node_id,
    }
    .intents(&func);

    let GraphIntent::AddNode {
        pos, node_id, node, ..
    } = add
    else {
        panic!("first intent adds the node, got {add:?}");
    };
    assert_eq!(pos, Vec2::new(180.0, -20.0), "offset clear of the port");
    assert_eq!(node.func_id(), Some(func.id()));
    assert!(matches!(
        wire,
        GraphIntent::SetInput { input, to: Some(Binding::Wire(src)) }
            if input == InputPort::new(node_id, PortId::new(1))
                && src == OutputPort::new(producer, PortId::new(3))
    ));
}

/// Double-clicking a wired input's port circle clears its binding — and
/// one click alone does not.
///
/// The signal is read off the glyph widget's own returned response. The circle senses its own
/// `Sense::CLICK` and consumes hits over its rect, so the cell around it never sees this click
/// (no bubbling): the test that the widget hands its clicks back rather than swallowing them.
#[test]
fn double_clicking_a_wired_input_port_circle_clears_its_binding() {
    use scenarium::{Binding, InputPort};

    use crate::core::document::internals::DocumentFixture;
    use crate::gui::pane::graph_ui::internals::CanvasHarness;
    use crate::gui::pane::graph_ui::wid;
    use crate::gui::port_ref::PortRef;

    let mut h = CanvasHarness::new(DocumentFixture::probes(2));
    let (producer, consumer) = (h.node(0), h.node(1));
    let input = InputPort::new(consumer, PortId::new(1));
    h.document_mut()
        .graphs
        .main_mut()
        .set_input_binding(input, Binding::wire(producer, PortId::new(1)));
    // Two frames so the circle has a widget id and a measured rect to aim at.
    h.prime(2);
    // Nudged off the circle's exact center: an input port hangs half its width past the node's
    // left edge, which puts its center a hair outside the body, on the canvas beneath. Still well
    // inside the glyph's 23.4 px box, and both clicks land on the same point.
    let at =
        h.ui.rect(wid::port_circle(
            GraphId::Main,
            PortRef::input(consumer, PortId::new(1)),
        ))
        .expect("the widget shows")
        .center()
            + Vec2::new(4.0, 0.0);

    h.ui.click(at);
    let first = h.frame();
    assert!(first.is_empty(), "one click is not a double: {first:?}");

    h.ui.click(at);
    let cleared = h.frame();
    assert!(
        matches!(
            cleared[..],
            [GraphIntent::SetInput { input: got, to: None }] if got == input
        ),
        "the second click clears the wired input, and nothing else: {cleared:?}"
    );
}

/// A ghost moves only to an input of its node that takes it: one that holds nothing and that
/// admission passes, as the gate would. On a `probe` pair, `b`'s ghost port 9 holds a wire from
/// `a`'s output 1, an `Int`: `b`'s free optional `Int` input 1 takes it; once bound it does not;
/// a constant `"loud"`, which an `Int` input refuses, goes nowhere; and a wire from a ghost node
/// goes nowhere until the node is back.
#[test]
fn a_ghost_moves_only_to_an_input_that_takes_it() {
    use scenarium::{Binding, ConstValue, GhostBinding, InputPort, InputState};

    use crate::core::document::internals::DocumentFixture;
    use crate::gui::graph_ctx::internals::GraphCtxFixture;

    #[derive(Debug, Clone, Copy)]
    enum Held {
        WireFromA,
        Loud,
        WireFromGhost,
    }
    let targets = |held: Held, bound: bool| {
        let mut fixture = DocumentFixture::probes(2);
        let [a, b] = [0, 1].map(|i| fixture.node(i));
        let ghost = fixture.ghost_node_at(Vec2::ZERO);
        let binding = match held {
            Held::WireFromA => Binding::wire(a, PortId::new(1)),
            Held::Loud => Binding::Const(ConstValue::String("loud".into())),
            Held::WireFromGhost => Binding::wire(ghost, PortId::new(1)),
        };
        let graph = fixture.document.graphs.main_mut();
        graph.set_input_state(
            InputPort::new(b, PortId::new(9)),
            InputState::Ghost(GhostBinding::from(binding)),
        );
        if bound {
            graph.set_input_binding(
                InputPort::new(b, PortId::new(1)),
                Binding::wire(a, PortId::new(1)),
            );
        }
        graph.validate().unwrap();
        let fixture = GraphCtxFixture::over(fixture);
        let node = fixture.graph_ctx().node(b).unwrap();
        let ghost = node
            .graph_ctx
            .body()
            .ghost_wiring()
            .binding(InputPort::new(b, PortId::new(9)))
            .unwrap();
        rehome_targets(node, ghost)
            .map(|input| input.port().port)
            .collect::<Vec<_>>()
    };
    assert_eq!(targets(Held::WireFromA, false), [PortId::new(1)]);
    assert_eq!(targets(Held::WireFromA, true), []);
    assert_eq!(targets(Held::Loud, false), []);
    assert_eq!(targets(Held::WireFromGhost, false), []);
}
