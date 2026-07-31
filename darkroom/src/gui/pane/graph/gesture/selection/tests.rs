use glam::{UVec2, Vec2};
use palantir::internals::UiHarness;
use palantir::{Configure, Modifiers, Panel, Sizing, Ui};
use scenarium::OutputTypes;

use crate::core::document::harness::one_func_library;
use crate::core::document::{Document, GraphView};
use crate::core::edit::intent::sink::Intents;
use crate::core::edit::intent::types::GraphIntent;
use crate::gui::pane::graph::GraphUI;
use crate::gui::pane::graph::harness::*;
use crate::gui::state::run_state::RunState;
use crate::gui::theme::Theme;

/// Escape cancels a rubber band: no `SetSelection`, and the next band
/// starts from a clean base.
///
/// The cancel is resolved once per frame by the canvas and handed to each
/// controller, so this also covers the wiring — a band that kept running
/// after Esc would commit on release. The second band is the half that
/// would break silently: `base` lives inside `RubberBand` now, and a
/// cancel that left it behind would union the abandoned drag's selection
/// into the next one.
#[test]
fn escape_cancels_a_rubber_band_and_leaves_no_residue() {
    use palantir::Key;

    let library = one_func_library();
    let probe = library.by_name("probe").expect("just added").clone();
    let mut doc = Document::default();
    let a = doc.graph.add_func_node(&probe);
    let b = doc.graph.add_func_node(&probe);
    doc.main_view = GraphView::for_graph(&doc.graph);
    // Placed by id, not by `spread`: this case cares *which* node the
    // second band reaches, and `spread` assigns by map iteration order.
    doc.main_view
        .item_placements
        .insert(a, Vec2::new(40.0, 40.0));
    doc.main_view
        .item_placements
        .insert(b, Vec2::new(400.0, 40.0));

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

    // Sweep bare canvas across both nodes, then cancel mid-drag.
    let empty = Vec2::new(20.0, 400.0);
    harness.press_at(empty);
    harness.frame(|ui| {
        draw(ui, &mut graph_ui);
    });
    harness.drag_to(Vec2::new(700.0, 60.0));
    harness.frame(|ui| {
        draw(ui, &mut graph_ui);
    });
    harness.key(Key::Escape);
    let cancelled = harness.frame_value(|ui| draw(ui, &mut graph_ui));
    assert!(
        cancelled.is_empty(),
        "a cancelled band commits nothing: {cancelled:?}"
    );
    harness.release_button(palantir::PointerButton::Left);
    let after = harness.frame_value(|ui| draw(ui, &mut graph_ui));
    assert!(
        after.is_empty(),
        "and the release of a cancelled band commits nothing either: {after:?}"
    );

    // A fresh band over `a` alone must select exactly `a` — if the
    // cancelled drag's `base` survived, `b` would ride along. `spread`
    // puts the nodes at x = 40 and x = 260, so a sweep stopping at x = 150
    // reaches the first and not the second; both bands start from the same
    // empty patch well below them.
    harness.press_at(empty);
    harness.frame(|ui| {
        draw(ui, &mut graph_ui);
    });
    harness.drag_to(Vec2::new(150.0, 100.0));
    harness.frame(|ui| {
        draw(ui, &mut graph_ui);
    });
    harness.release_button(palantir::PointerButton::Left);
    let emitted = harness.frame_value(|ui| draw(ui, &mut graph_ui));
    let intents = graph_intents(&emitted);
    assert!(
        matches!(
            intents[..],
            [GraphIntent::SetSelection { to }] if to.len() == 1 && to.contains(&a),
        ),
        "the next band selects only what it swept, with no residue from the \
         cancelled one: {intents:?} (b = {b:?})"
    );
}

/// A Shift-band commits the union of what it swept and what was already
/// selected, counting a node in both exactly once.
///
/// The overlap is the case that pins the sweep's shape: `base` arrives sorted
/// off the document's `BTreeSet`, the sweep appends in paint order, and a node
/// in both lands twice. `Selection::swept` binary-searches, so a sweep left
/// unsorted or undeduplicated answers *wrong* about what paints selected —
/// its debug assertion fires on every frame of this drag.
#[test]
fn a_shift_band_unions_with_the_committed_selection_counting_overlap_once() {
    let library = one_func_library();
    let probe = library.by_name("probe").expect("just added").clone();
    let mut doc = Document::default();
    let a = doc.graph.add_func_node(&probe);
    let b = doc.graph.add_func_node(&probe);
    doc.main_view = GraphView::for_graph(&doc.graph);
    doc.main_view
        .item_placements
        .insert(a, Vec2::new(40.0, 40.0));
    doc.main_view
        .item_placements
        .insert(b, Vec2::new(260.0, 40.0));
    // `a` is already selected, and the band below sweeps *both* — so `a`
    // reaches the swept buffer twice, once from the base and once from the
    // sweep.
    doc.main_view.selected.insert(a);

    let theme = Theme::default();
    let run_state = RunState::default();
    let mut harness = UiHarness::new(UVec2::new(1200, 800));
    let mut graph_ui = GraphUI::default();
    let draw = |ui: &mut Ui, graph_ui: &mut GraphUI| {
        let mut intents = Intents::default();
        let mut types = OutputTypes::default();
        let graph_ctx = graph_ctx_for(app(&theme, &library, &run_state), &doc, &mut types);
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

    harness.set_modifiers(Modifiers {
        shift: true,
        ..Modifiers::default()
    });
    harness.press_at(Vec2::new(20.0, 400.0));
    harness.frame(|ui| {
        draw(ui, &mut graph_ui);
    });
    // Two steps, not one: the band anchors where `drag.started()` first
    // fires, so a single jump would latch and finish at the same point and
    // sweep a zero-area rect.
    harness.drag_to(Vec2::new(20.0, 390.0));
    harness.frame(|ui| {
        draw(ui, &mut graph_ui);
    });
    harness.drag_to(Vec2::new(700.0, 60.0));
    harness.frame(|ui| {
        draw(ui, &mut graph_ui);
    });
    harness.release_button(palantir::PointerButton::Left);
    let emitted = harness.frame_value(|ui| draw(ui, &mut graph_ui));

    let intents = graph_intents(&emitted);
    // Both nodes, each once. `SetSelection` carries a `BTreeSet`, so the
    // count is what says the duplicate was folded before it got there.
    assert!(
        matches!(
            intents[..],
            [GraphIntent::SetSelection { to }]
                if to.len() == 2 && to.contains(&a) && to.contains(&b),
        ),
        "the shift band commits both nodes exactly once: {intents:?} (a = {a:?}, b = {b:?})"
    );
}
