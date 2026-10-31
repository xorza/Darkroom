use glam::Vec2;
use scenarium::Binding;

use crate::core::document::internals::DocumentFixture;
use crate::core::preview_sink;
use crate::gui::pane::graph_ui::internals::CanvasHarness;
use crate::gui::pane::graph_ui::wid;
use std::sync::Arc;

/// Ctrl+drag off an output port spawns a preview node already reading it, as
/// one batch — the one-gesture counterpart to the port menu's "Add preview".
///
/// Drives the real chord through the harness rather than calling the gesture
/// directly: the whole point of the modifier is that `WireUi` and
/// `PreviewDrag` can't both claim the same press, and only a real press
/// exercises that.
#[test]
fn ctrl_drag_off_an_output_spawns_a_preview_wired_to_it() {
    use palantir::{Modifiers, PointerButton};

    use crate::core::edit::graph_intent::GraphIntent;
    use crate::core::preview_sink::PreviewSink;
    use crate::gui::port_ref::PortRef;

    // The preview func is declared but not placed — the gesture is what
    // spawns a node from it.
    let mut fixture = DocumentFixture::probes(1);
    fixture
        .library
        .add(preview_sink::func(Arc::<PreviewSink>::default()));
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
    h.ui.press(wid::port_circle(out_port), PointerButton::Left);
    h.frame();
    // `first_drag_started` polls the *drag* edge, not the press, so the
    // pointer has to actually move past palantir's threshold.
    h.ui.drag_to(from + Vec2::new(90.0, 40.0));
    let spawned = h.frame();
    h.ui.set_modifiers(Modifiers::default());
    h.ui.release(PointerButton::Left);

    let adds: Vec<_> = spawned
        .iter()
        .filter(|intent| matches!(intent, GraphIntent::AddNode { .. }))
        .collect();
    assert_eq!(adds.len(), 1, "one preview spawned: {spawned:?}");
    let GraphIntent::AddNode { node_id, node, .. } = adds[0] else {
        unreachable!("filtered to AddNode");
    };
    assert!(
        node.func_id().is_some_and(preview_sink::is_func),
        "the spawned node is a preview"
    );
    assert!(
        spawned.iter().any(|intent| matches!(
            intent,
            GraphIntent::SetInput { input, to: Some(Binding::Wire(src)) }
                if input.node_id == *node_id
                    && src.node_id == producer
                    && src.port_idx == 0
        )),
        "and it is already reading the port the drag came off: {spawned:?}"
    );
}

/// Esc on a spawn-and-place drag takes the spawn back with the drag: the card is gone and there
/// is nothing to undo, where a drag released in place keeps its card. Esc cancels with the
/// spawn chord's Ctrl still held as well as with it let go.
#[test]
fn esc_on_a_preview_spawn_takes_the_card_back() {
    use palantir::{Key, Modifiers, PointerButton};

    use crate::core::preview_sink::PreviewSink;
    use crate::gui::port_ref::PortRef;

    let ctrl = Modifiers {
        ctrl: true,
        ..Default::default()
    };
    // `None` releases in place; `Some(mods)` presses Esc first with `mods` held.
    let spawn = |cancel: Option<Modifiers>| {
        let mut fixture = DocumentFixture::probes(1);
        fixture
            .library
            .add(preview_sink::func(Arc::<PreviewSink>::default()));
        let out_port = PortRef::output(fixture.node(0), 0);
        let mut h = CanvasHarness::new(fixture);
        h.prime(2);
        h.ui.set_modifiers(ctrl);
        let from = h.port_center(out_port);
        h.ui.press(wid::port_circle(out_port), PointerButton::Left);
        h.frame_applied();
        h.ui.drag_to(from + Vec2::new(90.0, 40.0));
        h.frame_applied();
        if let Some(mods) = cancel {
            h.ui.set_modifiers(mods);
            h.frame_applied();
            h.ui.key(Key::Escape);
            h.frame_applied();
        }
        h.ui.release(PointerButton::Left);
        h.frame_applied();
        (h.document().graph.len(), h.ctx.open.can_undo())
    };
    assert_eq!(spawn(None), (2, true), "a placed card stays, undoable");
    for mods in [Modifiers::default(), ctrl] {
        assert_eq!(
            spawn(Some(mods)),
            (1, false),
            "cancelled under {mods:?}: nothing left"
        );
    }
}
