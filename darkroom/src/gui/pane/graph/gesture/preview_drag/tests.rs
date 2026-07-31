use glam::Vec2;
use scenarium::{Binding, NodeKind};

use crate::core::document::harness::DocFixture;
use crate::gui::pane::graph::harness::CanvasHarness;
use crate::gui::pane::graph::node::port_row::port_circle_wid;

/// Ctrl+drag off an output port spawns a preview node already reading it, as
/// one batch — the one-gesture counterpart to the port menu's "Add preview".
///
/// Drives the real chord through the harness rather than calling the gesture
/// directly: the whole point of the modifier is that `ConnectionUI` and
/// `PreviewDrag` can't both claim the same press, and only a real press
/// exercises that.
#[test]
fn ctrl_drag_off_an_output_spawns_a_preview_wired_to_it() {
    use palantir::{Modifiers, PointerButton};

    use crate::core::document::PortRef;
    use crate::core::edit::intent::types::GraphIntent;
    use crate::core::preview::{PreviewSink, preview_func};

    // The preview func is declared but not placed — the gesture is what
    // spawns a node from it.
    let mut fixture = DocFixture::probes(1);
    fixture
        .library
        .add(preview_func(std::sync::Arc::<PreviewSink>::default()));
    let producer = fixture.node(0);
    let out_port = PortRef::output(producer, 0);

    // Two frames to record the node so its output circle has a widget id and
    // `CanvasGeometry` a measured center; the gesture refuses an unmeasured
    // port outright.
    let mut h = CanvasHarness::new(fixture);
    h.prime(2);

    // Ctrl held, press on the output circle, then drag: the press frame is
    // where `PreviewDrag` latches.
    h.ui.set_modifiers(Modifiers {
        ctrl: true,
        ..Default::default()
    });
    let from = h.port_center(out_port);
    h.ui.press_on(port_circle_wid(out_port));
    h.frame();
    // `first_drag_started` polls the *drag* edge, not the press, so the
    // pointer has to actually move past palantir's threshold.
    h.ui.drag_to(from + Vec2::new(90.0, 40.0));
    let spawned = h.frame();
    h.ui.set_modifiers(Modifiers::default());
    h.ui.release_button(PointerButton::Left);

    // The harness carries the pane assertion: the spawn is raised against the
    // port's own pane, not whichever one happens to be focused.
    let adds: Vec<_> = spawned
        .iter()
        .filter(|intent| matches!(intent, GraphIntent::AddNode { .. }))
        .collect();
    assert_eq!(adds.len(), 1, "one preview spawned: {spawned:?}");
    let GraphIntent::AddNode { node_id, node, .. } = adds[0] else {
        unreachable!("filtered to AddNode");
    };
    assert!(
        matches!(node.kind, NodeKind::Func(id) if crate::core::preview::is_preview(id)),
        "the spawned node is a preview"
    );
    assert!(
        spawned.iter().any(|intent| matches!(
            intent,
            GraphIntent::SetInput { input, to: Some(Binding::Bind(src)) }
                if input.node_id == *node_id
                    && src.node_id == producer
                    && src.port_idx == 0
        )),
        "and it is already reading the port the drag came off: {spawned:?}"
    );
}
