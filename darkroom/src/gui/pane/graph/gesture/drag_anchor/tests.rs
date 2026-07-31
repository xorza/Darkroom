use glam::{UVec2, Vec2};
use palantir::internals::UiHarness;
use palantir::{Configure, Panel, Sizing, Ui};
use scenarium::OutputTypes;

use crate::core::document::{Document, GraphView};
use crate::core::edit::intent::sink::Intents;
use crate::core::edit::intent::types::GraphIntent;
use crate::gui::pane::graph::GraphUI;
use crate::gui::pane::graph::harness::*;
use crate::gui::pane::graph::node::node_widget_id;
use crate::gui::state::run_state::RunState;
use crate::gui::theme::Theme;

/// A drag on a node body moves that node, by the pointer's travel.
///
/// The drag *latch* is the one thing `CanvasHits` resolves for the record
/// rather than for an input pass: `NodeUI::draw_one` no longer polls the
/// node's own handles, it reads the handle the sweep found. So this drives
/// a real press-and-travel through the harness — nothing else in the suite
/// latches a body drag, and a latch that silently stopped firing would
/// leave every node unmovable with the whole rest of the canvas green.
#[test]
fn a_body_drag_moves_the_node_by_the_pointers_travel() {
    let library = one_func_library();
    let probe = library.by_name("probe").expect("just added").clone();

    let mut doc = Document::default();
    let dragged = doc.graph.add_func_node(&probe);
    let bystander = doc.graph.add_func_node(&probe);
    doc.main_view = GraphView::for_graph(&doc.graph);
    spread(&mut doc.main_view);
    let start = doc.main_view.item_placements[&dragged];

    let theme = Theme::default();
    let run_state = RunState::default();
    let mut harness = UiHarness::new(UVec2::new(1200, 800));
    let mut graph_ui = GraphUI::default();

    let draw = |ui: &mut Ui, graph_ui: &mut GraphUI| {
        let ctx = app(&theme, &library, &run_state);
        let mut intents = Intents::default();
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

    for _ in 0..2 {
        harness.frame(|ui| {
            draw(ui, &mut graph_ui);
        });
    }

    // Press the body, then travel past the drag threshold. The sweep sees
    // the latch on the frame after the travel; the record consumes it.
    let grab = harness.center_of(node_widget_id(dragged));
    harness.press_at(grab);
    harness.frame(|ui| {
        draw(ui, &mut graph_ui);
    });
    let travel = Vec2::new(37.0, -21.0);
    harness.drag_to(grab + travel);
    harness.frame(|ui| {
        draw(ui, &mut graph_ui);
    });

    // Next frame, `NodeUI::prepass` advances the anchor the record latched.
    let emitted = harness.frame_value(|ui| draw(ui, &mut graph_ui));
    let intents = graph_intents(&emitted);
    let moves = intents
        .iter()
        .find_map(|intent| match intent {
            GraphIntent::MoveSelection { grabbed, moves } => Some((*grabbed, moves)),
            _ => None,
        })
        .unwrap_or_else(|| panic!("a body drag must emit a MoveSelection: {intents:?}"));
    assert_eq!(moves.0, dragged, "the grabbed node is the one pressed");
    // Target = press-frame position + cumulative travel, so the node lands
    // exactly where the pointer took it — and the untouched node stays out
    // of the batch, since the grab selected only the node under it.
    assert_eq!(
        moves.1.as_slice(),
        &[(dragged, start + travel)],
        "the drag moves only the grabbed node, to its start plus the travel"
    );
    assert!(
        !moves.1.iter().any(|(id, _)| *id == bystander),
        "an unselected neighbour is not dragged along"
    );
}
