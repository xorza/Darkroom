use glam::UVec2;
use palantir::internals::UiHarness;
use palantir::{Configure, Panel, Sizing, Ui};
use scenarium::OutputTypes;

use crate::core::document::harness::{one_func_library, spread};
use crate::core::document::{Document, GraphView};
use crate::core::edit::intent::sink::Intents;
use crate::core::edit::intent::types::GraphIntent;
use crate::gui::pane::graph::GraphUI;
use crate::gui::pane::graph::harness::*;
use crate::gui::pane::graph::node::node_widget_id;
use crate::gui::state::run_state::RunState;
use crate::gui::theme::Theme;

/// A node-body right-click selects the node it landed on before the menu
/// opens, so whatever the user picks next acts on a coherent set.
///
/// It comes out of the closure the shared trigger scan takes — "which of this
/// node's widgets opens the menu" — so it's checked through a real click
/// rather than by calling it.
#[test]
fn a_node_body_right_click_selects_the_node_it_landed_on() {
    let library = one_func_library();
    let probe = library.by_name("probe").expect("just added").clone();

    // Two nodes, so the assertion that exactly one ends up selected has
    // something to exclude.
    let mut doc = Document::default();
    let func = doc.graph.add_func_node(&probe);
    doc.graph.add_func_node(&probe);
    doc.main_view = GraphView::for_graph(&doc.graph);
    spread(&mut doc.main_view);

    let theme = Theme::default();
    let run_state = RunState::default();
    let mut harness = UiHarness::new(UVec2::new(1600, 900));
    let mut graph_ui = GraphUI::default();

    let draw = |ui: &mut Ui, graph_ui: &mut GraphUI| {
        let ctx = app(&theme, &library, &run_state);
        let mut intents = Intents::default();
        // Navigation phase first — the sweep runs before the tab set settles.
        let mut types = OutputTypes::default();
        let graph_ctx = graph_ctx_for(ctx, &doc, &mut types);
        graph_ui.scan_hits(ui, Some(graph_ctx));
        graph_ui.prepass(ui, graph_ctx, &mut intents);
        Panel::vstack()
            .id_salt("pane")
            .size((Sizing::FILL, Sizing::FILL))
            .show(ui, |ui| {
                graph_ui.draw(ui, graph_ctx, &mut intents);
            });
        intents.drain().collect::<Vec<_>>()
    };

    // Two frames so every node body has recorded and carries a hit-testable
    // rect for the clicks below.
    harness.frame(|ui| {
        draw(ui, &mut graph_ui);
    });
    harness.frame(|ui| {
        draw(ui, &mut graph_ui);
    });

    let on_func = harness.center_of(node_widget_id(func));
    harness.right_click_at(on_func);
    let emitted = harness.frame_value(|ui| draw(ui, &mut graph_ui));
    let intents = graph_intents(&emitted);
    assert!(
        matches!(
            intents[..],
            [GraphIntent::SetSelection { to }] if to.len() == 1 && to.contains(&func),
        ),
        "the right-click selects exactly the node it opened on: {intents:?}"
    );
}
