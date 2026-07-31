use glam::{UVec2, Vec2};
use palantir::internals::UiHarness;
use palantir::{Configure, Panel, Sizing, Ui};
use scenarium::{Binding, NodeKind, OutputTypes};

use crate::core::document::harness::{one_func_library, spread};
use crate::core::document::{Document, GraphView};
use crate::core::edit::intent::sink::Intents;
use crate::gui::pane::graph::GraphUI;
use crate::gui::pane::graph::harness::*;
use crate::gui::pane::graph::node::port_row::port_circle_wid;
use crate::gui::state::run_state::RunState;
use crate::gui::theme::Theme;

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

    use crate::core::document::{PortKind, PortRef};
    use crate::core::edit::intent::types::GraphIntent;
    use crate::core::preview::{PreviewSink, preview_func};

    let mut library = one_func_library();
    library.add(preview_func(std::sync::Arc::<PreviewSink>::default()));
    let probe = library.by_name("probe").expect("just added").clone();

    let mut doc = Document::default();
    let producer = doc.graph.add_func_node(&probe);
    doc.main_view = GraphView::for_graph(&doc.graph);
    spread(&mut doc.main_view);

    let mut harness = UiHarness::new(UVec2::new(1200, 800));
    let theme = Theme::default();
    let run_state = RunState::default();
    let mut graph_ui = GraphUI::default();
    let out_port = PortRef {
        node_id: producer,
        kind: PortKind::Output,
        port_idx: 0,
    };

    // One frame to record the node so its output circle has a widget id and
    // `CanvasGeometry` a measured center; the gesture refuses an unmeasured
    // port outright.
    let draw = |ui: &mut Ui, graph_ui: &mut GraphUI| {
        let ctx = app(&theme, &library, &run_state);
        let mut intents = Intents::default();
        let mut types = OutputTypes::default();
        let graph_ctx = graph_ctx_for(ctx, &doc, &mut types);
        graph_ui.prepass(ui, graph_ctx, &mut intents);
        Panel::vstack()
            .id_salt("pane")
            .size((Sizing::FILL, Sizing::FILL))
            .show(ui, |ui| {
                graph_ui.draw(ui, graph_ctx, &mut intents);
            });
        intents.drain().collect::<Vec<_>>()
    };

    harness.frame(|ui| {
        draw(ui, &mut graph_ui);
    });
    harness.frame(|ui| {
        draw(ui, &mut graph_ui);
    });

    // Ctrl held, press on the output circle, then drag: the press frame is
    // where `PreviewDrag` latches.
    harness.set_modifiers(Modifiers {
        ctrl: true,
        ..Default::default()
    });
    let circle = port_circle_wid(out_port);
    harness.press_on(circle);
    harness.frame(|ui| {
        draw(ui, &mut graph_ui);
    });
    // `first_drag_started` polls the *drag* edge, not the press, so the
    // pointer has to actually move past palantir's threshold.
    let from = harness
        .ui()
        .response_for(circle)
        .layout_rect
        .expect("recorded")
        .center();
    harness.drag_to(from + Vec2::new(90.0, 40.0));
    let spawned = harness.frame_value(|ui| draw(ui, &mut graph_ui));
    harness.set_modifiers(Modifiers::default());
    harness.release_button(PointerButton::Left);

    // The helper carries the pane assertion: the spawn is raised against the
    // port's own pane, not whichever one happens to be focused.
    let spawned = graph_intents(&spawned);
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
