//! The floating toolbar pinned to the graph view's top-left corner: a
//! run/cancel toggle and an event-loop start/stop toggle side by side on one
//! chrome pill — drawn only on the main graph's pane, since both act on the
//! whole document — with three view-framing buttons (reset view, show all,
//! show selected) stacked beneath on a second pill that every pane carries.
//! The frosted pills keep the toolbar legible over both the canvas and any
//! node under it; the buttons are opaque chips raised off the pill. All carry
//! hover tooltips; the toggles paint "toggled" while their action is in flight
//! and map to an [`AppCommand`], while the framing buttons emit an
//! `Intent::SetViewport` directly.

use glam::Vec2;
use palantir::{
    Align, Color, Configure, HAlign, Panel, Rect, Shape, Sizing, Spacing, Ui, VAlign, WidgetId,
};

use crate::core::document::GraphRef;
use crate::core::edit::intent::sink::Intents;
use crate::gui::app::AppContext;
use crate::gui::app::commands::AppCommand;
use crate::gui::app::commands::run::RunCommand;
use crate::gui::canvas::geometry::CanvasGeometry;
use crate::gui::canvas::pan_zoom::{self, ViewAction};
use crate::gui::scene::Pane;
use crate::gui::widgets::support::{dot, filled_rect, frame, stroked_rect};
use crate::gui::widgets::toolbar::{BUTTON_GAP, Chip, TOOLBAR_MARGIN, pill};

/// Toolbar chip ids are keyed by the pane they sit on: every visible
/// graph pane draws its own toolbar, so a shared id would record the same
/// widget several times in one frame.
fn run_button_wid(graph: GraphRef) -> WidgetId {
    WidgetId::from_hash(("darkroom.graph.run_button", graph))
}

fn events_button_wid(graph: GraphRef) -> WidgetId {
    WidgetId::from_hash(("darkroom.graph.events_button", graph))
}

fn reset_view_wid(graph: GraphRef) -> WidgetId {
    WidgetId::from_hash(("darkroom.graph.reset_view_button", graph))
}

fn show_all_wid(graph: GraphRef) -> WidgetId {
    WidgetId::from_hash(("darkroom.graph.show_all_button", graph))
}

fn show_selected_wid(graph: GraphRef) -> WidgetId {
    WidgetId::from_hash(("darkroom.graph.show_selected_button", graph))
}

/// Draw the toolbar over the graph view's top-left corner. Returns the
/// [`AppCommand`] a run/events click implies — always `None` off the main
/// pane, which draws no run pill; view-framing clicks push an
/// `Intent::SetViewport` onto `out` instead. It hit-tests above the canvas
/// (drawn after it), so a click on a button never starts a pan.
pub(crate) fn show(
    ui: &mut Ui,
    ctx: &AppContext<'_>,
    graph: Pane<'_>,
    geometry: &CanvasGeometry,
    out: &mut Intents,
) -> Option<AppCommand> {
    let target = graph.target();
    let mut command = None;
    Panel::vstack()
        .id_salt(("graph_toolbar", target))
        .size((Sizing::HUG, Sizing::HUG))
        .align(Align::new(HAlign::Left, VAlign::Top))
        .child_align(Align::new(HAlign::Left, VAlign::Top))
        .margin(Spacing::new(TOOLBAR_MARGIN, TOOLBAR_MARGIN, 0.0, 0.0))
        .gap(BUTTON_GAP)
        .show(ui, |ui| {
            // Top row: run/cancel + event-loop toggles, side by side on their
            // own chrome pill. Both compile and run the document root rather
            // than the pane they were clicked from, so a subgraph pane must
            // not offer them — its button would silently act on another graph.
            if target == GraphRef::Main {
                pill(
                    ui,
                    ctx.theme,
                    Panel::hstack().id_salt(("graph_toolbar_run", target)),
                    |ui| {
                        // Run / cancel: toggled while a one-shot run is in
                        // flight.
                        let running = ctx.run_state.activity.is_executing();
                        let run_tip = if running { "Cancel run" } else { "Run" };
                        // Run is the one primary action in the cluster — it
                        // alone idles with the accent glyph; the event-loop
                        // toggle sits muted beside it like the framing
                        // buttons below.
                        if Chip::new(run_button_wid(target), run_tip)
                            .toggled(running)
                            .idle_glyph(ctx.theme.colors.exec_executed_glow)
                            .toggled_fill(ctx.theme.colors.exec_running_glow)
                            .show(ui, ctx.theme, draw_play)
                        {
                            command = Some(if running {
                                AppCommand::Run(RunCommand::Cancel)
                            } else {
                                AppCommand::Run(RunCommand::Once)
                            });
                        }
                        // Event loop start / stop: toggled while the loop runs.
                        let event_loop_active = ctx.run_state.activity.event_loop_active();
                        let events_tip = if event_loop_active {
                            "Stop events"
                        } else {
                            "Start events"
                        };
                        if Chip::new(events_button_wid(target), events_tip)
                            .toggled(event_loop_active)
                            .toggled_fill(ctx.theme.colors.exec_running_glow)
                            .show(ui, ctx.theme, draw_play_bar)
                        {
                            command = Some(if event_loop_active {
                                AppCommand::Run(RunCommand::StopEvents)
                            } else {
                                AppCommand::Run(RunCommand::StartEvents)
                            });
                        }
                    },
                );
            }
            // View-framing actions, stacked under the run row on their own
            // chrome pill. Each emits a `SetViewport` intent (undoable), so they
            // ride the same path as a manual pan/zoom rather than mutating the
            // viewport out of band.
            let framing = Panel::vstack()
                .id_salt(("graph_toolbar_framing", target))
                .child_align(Align::new(HAlign::Left, VAlign::Top));
            pill(ui, ctx.theme, framing, |ui| {
                if Chip::new(reset_view_wid(target), "Reset view").show(ui, ctx.theme, draw_reset) {
                    out.extend(
                        target,
                        pan_zoom::view_action_intent(ui, geometry, graph, ViewAction::Reset),
                    );
                }
                if Chip::new(show_all_wid(target), "Show all").show(ui, ctx.theme, draw_show_all) {
                    out.extend(
                        target,
                        pan_zoom::view_action_intent(ui, geometry, graph, ViewAction::ShowAll),
                    );
                }
                if Chip::new(show_selected_wid(target), "Show selected").show(
                    ui,
                    ctx.theme,
                    draw_show_selected,
                ) {
                    out.extend(
                        target,
                        pan_zoom::view_action_intent(ui, geometry, graph, ViewAction::ShowSelected),
                    );
                }
            });
        });
    command
}

/// A right-pointing play triangle (run once), optically centered in the box.
fn draw_play(ui: &mut Ui, s: f32, color: Color) {
    ui.add_shape(
        Shape::triangle(
            Vec2::new(s * 0.38, s * 0.30),
            Vec2::new(s * 0.38, s * 0.70),
            Vec2::new(s * 0.70, s * 0.50),
        )
        .fill(color),
    );
}

/// `|>` — a vertical bar then a play triangle (start the event loop).
fn draw_play_bar(ui: &mut Ui, s: f32, color: Color) {
    // The bar.
    filled_rect(
        ui,
        Rect::new(s * 0.28, s * 0.30, s * 0.085, s * 0.40),
        1.0,
        color,
    );
    // The play triangle, just to its right.
    ui.add_shape(
        Shape::triangle(
            Vec2::new(s * 0.46, s * 0.30),
            Vec2::new(s * 0.46, s * 0.70),
            Vec2::new(s * 0.74, s * 0.50),
        )
        .fill(color),
    );
}

/// Reset view: a target ring with a center dot (recenter to 1:1).
fn draw_reset(ui: &mut Ui, s: f32, color: Color) {
    let d = s * 0.52;
    let o = (s - d) * 0.5;
    stroked_rect(ui, Rect::new(o, o, d, d), d * 0.5, color, s * 0.06);
    dot(ui, s * 0.5, s * 0.5, s * 0.075, color);
}

/// Show all: a frame enclosing a 2×2 field of dots (fit every node).
fn draw_show_all(ui: &mut Ui, s: f32, color: Color) {
    frame(ui, s, color);
    let r = s * 0.055;
    let near = s * 0.5 - s * 0.11;
    let far = s * 0.5 + s * 0.11;
    for &cy in &[near, far] {
        for &cx in &[near, far] {
            dot(ui, cx, cy, r, color);
        }
    }
}

/// Show selected: a frame enclosing one filled square (fit the selection).
fn draw_show_selected(ui: &mut Ui, s: f32, color: Color) {
    frame(ui, s, color);
    let inner = s * 0.24;
    let o = (s - inner) * 0.5;
    filled_rect(ui, Rect::new(o, o, inner, inner), s * 0.04, color);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::edit::intent::sink::Queued;
    use crate::core::edit::intent::types::Intent;
    use crate::gui::canvas::outer_canvas_widget_id;
    use crate::gui::run_state::RunState;
    use crate::gui::scene::{GraphProjection, Scene, SceneSource};
    use crate::gui::theme::Theme;
    use glam::UVec2;
    use palantir::internals::UiHarness;
    use scenarium::{GraphDef, GraphId, Library};

    use crate::core::document::Document;
    use crate::gui::scene::Frame;

    /// Run and the event loop compile the *document root*, so a subgraph
    /// pane offering them would silently act on another graph — the run
    /// pill belongs to the main pane alone. The framing pill is genuinely
    /// per-pane and must survive on both, and must frame *its own* pane:
    /// a `SetViewport` is valid against any graph, so a mistargeted one
    /// doesn't fail — it pans the other pane's camera and records an undo
    /// entry there. Drawn as two panes in one frame, which is the
    /// arrangement that surfaced both bugs.
    #[test]
    fn the_run_pill_is_main_only_while_framing_is_per_pane() {
        let def_id = GraphId::unique();
        let local = GraphRef::Local(def_id);
        let mut doc = Document::default();
        doc.graph.insert_graph(def_id, GraphDef::new("Adder"));
        assert!(doc.ensure_sub_view(def_id), "the def was just inserted");
        let theme = Theme::default();
        let library = Library::default();
        let run_state = RunState::default();
        let ctx = AppContext {
            theme: &theme,
            library: &library,
            run_state: &run_state,
            status_error: None,
            process_memory: 0,
        };
        let geometry = CanvasGeometry::default();
        let mut intents = Intents::default();
        let mut scene = Scene::default();
        let mut harness = UiHarness::new(UVec2::splat(800));

        let mut draw = |ui: &mut Ui| {
            scene.rebuild(
                ui,
                &library,
                &run_state,
                [
                    GraphProjection {
                        target: GraphRef::Main,
                        source: SceneSource::Entry(&doc.graph),
                        view: &doc.main_view,
                    },
                    GraphProjection {
                        target: local,
                        source: SceneSource::Def(doc.graph.find_graph(def_id).unwrap()),
                        view: doc.view(local).unwrap(),
                    },
                ],
            );
            for target in [GraphRef::Main, local] {
                let graph = Frame {
                    scene: &scene,
                    doc: &doc,
                }
                .pane(target)
                .expect("projected");
                // The framing actions size their fit against the pane's outer
                // canvas, which `GraphUI::draw` records around this toolbar;
                // stand in for it so a click resolves to an intent.
                Panel::canvas()
                    .id(outer_canvas_widget_id(target))
                    .size((Sizing::FILL, Sizing::FILL))
                    .show(ui, |ui| {
                        let command = show(ui, &ctx, graph, &geometry, &mut intents);
                        assert!(command.is_none(), "no run/cancel chip was clicked");
                    });
            }
        };
        harness.frame(&mut draw);

        assert!(
            harness.rect(run_button_wid(GraphRef::Main)).is_some(),
            "the main pane keeps its run toggle"
        );
        assert!(
            harness.rect(events_button_wid(GraphRef::Main)).is_some(),
            "the main pane keeps its event-loop toggle"
        );
        assert!(
            harness.rect(run_button_wid(local)).is_none(),
            "a subgraph pane draws no run toggle"
        );
        assert!(
            harness.rect(events_button_wid(local)).is_none(),
            "a subgraph pane draws no event-loop toggle"
        );
        for target in [GraphRef::Main, local] {
            for (wid, what) in [
                (reset_view_wid(target), "reset view"),
                (show_all_wid(target), "show all"),
                (show_selected_wid(target), "show selected"),
            ] {
                assert!(
                    harness.rect(wid).is_some(),
                    "{what} frames the pane it was clicked on, so every pane \
                     carries it — missing on {target:?}"
                );
            }
        }

        // And it frames that pane, not the root: click the subgraph pane's
        // "Reset view" and read what the sink queued. Reset rather than the
        // two fitting actions, which need content to fit and these panes are
        // empty.
        harness.click_on(reset_view_wid(local));
        // By value: the last frame, and the one that releases `intents`.
        harness.frame(draw);
        assert!(
            matches!(
                intents.drain().collect::<Vec<_>>()[..],
                [Queued::Scoped {
                    target,
                    intent: Intent::SetViewport { .. },
                }] if target == local,
            ),
            "the framing click must move its own pane's viewport",
        );
    }
}
