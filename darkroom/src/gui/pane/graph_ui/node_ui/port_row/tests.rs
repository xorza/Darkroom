use super::*;
use scenarium::FsPathConfig;

/// The type line of a port tooltip. A path spells out its picker mode and
/// the extensions it accepts; everything else defers to the library's own
/// name for the type, so a tooltip and a palette entry can't disagree.
#[test]
fn type_label_spells_out_a_paths_mode_and_extension_list() {
    let library = Library::default();
    let label = |data_type: &DataType| {
        TypeLabel {
            library: &library,
            data_type,
        }
        .to_string()
    };
    let path = |mode, exts: &[&str]| {
        DataType::FsPath(Arc::new(FsPathConfig::with_extensions(
            mode,
            exts.iter().map(|e| (*e).to_owned()).collect(),
        )))
    };

    // Scalars and the untyped boundary placeholder are the library's names.
    assert_eq!(label(&DataType::Any), "any");
    assert_eq!(label(&DataType::Float), "float");

    // Every mode names itself, and an empty list adds no parenthetical.
    assert_eq!(label(&path(FsPathMode::Dir, &[])), "path · directory");
    assert_eq!(label(&path(FsPathMode::ExistingFile, &[])), "path · file");
    assert_eq!(label(&path(FsPathMode::ExistingFiles, &[])), "path · files");
    assert_eq!(label(&path(FsPathMode::NewFile, &[])), "path · save path");

    // One extension, then several: the separator appears only *between* them.
    assert_eq!(
        label(&path(FsPathMode::ExistingFile, &["fit"])),
        "path · file (fit)"
    );
    assert_eq!(
        label(&path(FsPathMode::ExistingFiles, &["fit", "fits", "raf"])),
        "path · files (fit, fits, raf)"
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
    let port = OutputPort::new(producer, 2);
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
            if input == InputPort::new(node_id, 0)
                && src == OutputPort::new(producer, 2)
    ));
}

/// Double-clicking a wired input's port circle clears its binding — and
/// one click alone does not.
///
/// The signal is read off the glyph widget's own returned response, not a
/// `response_for` probe of its id after the fact. The circle senses its
/// own `Sense::CLICK` and consumes hits over its rect, so the cell around
/// it never sees this click (no bubbling) and only the glyph's answer can
/// carry it — which makes this the test that the widget hands its clicks
/// back rather than swallowing them.
#[test]
fn double_clicking_a_wired_input_port_circle_clears_its_binding() {
    use scenarium::{Binding, InputPort};

    use crate::core::document::internals::DocumentFixture;
    use crate::gui::pane::graph_ui::internals::CanvasHarness;
    use crate::gui::pane::graph_ui::wid;
    use crate::gui::port_ref::PortRef;

    let mut h = CanvasHarness::new(DocumentFixture::probes(2));
    let (producer, consumer) = (h.node(0), h.node(1));
    let input = InputPort::new(consumer, 0);
    h.document_mut()
        .graph
        .set_input_binding(input, Binding::wire(producer, 0));
    // Two frames so the circle has a widget id and a measured rect to aim at.
    h.prime(2);
    // Nudged off the circle's exact center: an input port hangs half its
    // width past the node's left edge, which puts its center a hair
    // *outside* the body and on the canvas beneath. Still well inside the
    // glyph — its box is 23.4 px across — and both clicks land on the same
    // point, so the double-click still registers.
    let at =
        h.ui.rect(wid::port_circle(PortRef::input(consumer, 0)))
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
