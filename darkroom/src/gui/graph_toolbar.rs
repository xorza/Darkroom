//! The floating toolbar pinned to the graph view's top-left corner: a
//! run/cancel toggle and an event-loop start/stop toggle side by side on one
//! chrome pill, with three view-framing buttons (reset view, show all, show
//! selected) stacked beneath on a second pill. The frosted pills keep the
//! toolbar legible over both the canvas and any node under it; the buttons are
//! opaque chips raised off the pill. All carry hover tooltips; the toggles
//! paint "toggled" while their action is in flight and map to an [`AppCommand`],
//! while the framing buttons emit an `Intent::SetViewport` directly.

use aperture::{
    Align, Color, Configure, HAlign, Panel, Rect, Shape, Sizing, Spacing, Stroke, Ui, VAlign,
    WidgetId,
};
use glam::Vec2;

use crate::core::edit::intent::Intent;
use crate::gui::app::AppContext;
use crate::gui::app::commands::AppCommand;
use crate::gui::app::commands::run::RunCommand;
use crate::gui::canvas::geometry::CanvasGeometry;
use crate::gui::canvas::pan_zoom::{self, ViewAction};
use crate::gui::scene::Scene;
use crate::gui::widgets::support::{dot, filled_rect, frame, stroked_rect};
use crate::gui::widgets::toolbar::{BUTTON_GAP, Chip, TOOLBAR_MARGIN, pill};

fn run_button_wid() -> WidgetId {
    WidgetId::from_hash("darkroom.graph.run_button")
}

fn events_button_wid() -> WidgetId {
    WidgetId::from_hash("darkroom.graph.events_button")
}

fn reset_view_wid() -> WidgetId {
    WidgetId::from_hash("darkroom.graph.reset_view_button")
}

fn show_all_wid() -> WidgetId {
    WidgetId::from_hash("darkroom.graph.show_all_button")
}

fn show_selected_wid() -> WidgetId {
    WidgetId::from_hash("darkroom.graph.show_selected_button")
}

/// Draw the toolbar over the graph view's top-left corner. Returns the
/// [`AppCommand`] a run/events click implies; view-framing clicks push an
/// `Intent::SetViewport` onto `out` instead. It hit-tests above the canvas
/// (drawn after it), so a click on a button never starts a pan.
pub(crate) fn show(
    ui: &mut Ui,
    ctx: &AppContext<'_>,
    scene: &Scene,
    geometry: &CanvasGeometry,
    out: &mut Vec<Intent>,
) -> Option<AppCommand> {
    let mut command = None;
    Panel::vstack()
        .id_salt("graph_toolbar")
        .size((Sizing::Hug, Sizing::Hug))
        .align(Align::new(HAlign::Left, VAlign::Top))
        .child_align(Align::new(HAlign::Left, VAlign::Top))
        .margin(Spacing::new(TOOLBAR_MARGIN, TOOLBAR_MARGIN, 0.0, 0.0))
        .gap(BUTTON_GAP)
        .show(ui, |ui| {
            // Top row: run/cancel + event-loop toggles, side by side on their
            // own chrome pill.
            pill(
                ui,
                ctx.theme,
                Panel::hstack().id_salt("graph_toolbar_run"),
                |ui| {
                    // Run / cancel: toggled while a one-shot run is in flight.
                    let running = ctx.run_state.is_running();
                    let run_tip = if running { "Cancel run" } else { "Run" };
                    // Run is the one primary action in the cluster — it alone
                    // idles with the accent glyph; the event-loop toggle sits
                    // muted beside it like the framing buttons below.
                    if Chip::new(run_button_wid(), run_tip)
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
                    let events_tip = if ctx.events_running {
                        "Stop events"
                    } else {
                        "Start events"
                    };
                    if Chip::new(events_button_wid(), events_tip)
                        .toggled(ctx.events_running)
                        .toggled_fill(ctx.theme.colors.exec_running_glow)
                        .show(ui, ctx.theme, draw_play_bar)
                    {
                        command = Some(if ctx.events_running {
                            AppCommand::Run(RunCommand::StopEvents)
                        } else {
                            AppCommand::Run(RunCommand::StartEvents)
                        });
                    }
                },
            );
            // View-framing actions, stacked under the run row on their own
            // chrome pill. Each emits a `SetViewport` intent (undoable), so they
            // ride the same path as a manual pan/zoom rather than mutating the
            // viewport out of band.
            let framing = Panel::vstack()
                .id_salt("graph_toolbar_framing")
                .child_align(Align::new(HAlign::Left, VAlign::Top));
            pill(ui, ctx.theme, framing, |ui| {
                if Chip::new(reset_view_wid(), "Reset view").show(ui, ctx.theme, draw_reset) {
                    out.extend(pan_zoom::view_action_intent(
                        ui,
                        geometry,
                        scene,
                        ViewAction::Reset,
                    ));
                }
                if Chip::new(show_all_wid(), "Show all").show(ui, ctx.theme, draw_show_all) {
                    out.extend(pan_zoom::view_action_intent(
                        ui,
                        geometry,
                        scene,
                        ViewAction::ShowAll,
                    ));
                }
                if Chip::new(show_selected_wid(), "Show selected").show(
                    ui,
                    ctx.theme,
                    draw_show_selected,
                ) {
                    out.extend(pan_zoom::view_action_intent(
                        ui,
                        geometry,
                        scene,
                        ViewAction::ShowSelected,
                    ));
                }
            });
        });
    command
}

/// A right-pointing play triangle (run once), optically centered in the box.
fn draw_play(ui: &mut Ui, s: f32, color: Color) {
    ui.add_shape(Shape::Triangle {
        a: Vec2::new(s * 0.38, s * 0.30),
        b: Vec2::new(s * 0.38, s * 0.70),
        c: Vec2::new(s * 0.70, s * 0.50),
        radius: 0.0,
        fill: color.into(),
        stroke: Stroke::ZERO,
    });
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
    ui.add_shape(Shape::Triangle {
        a: Vec2::new(s * 0.46, s * 0.30),
        b: Vec2::new(s * 0.46, s * 0.70),
        c: Vec2::new(s * 0.74, s * 0.50),
        radius: 0.0,
        fill: color.into(),
        stroke: Stroke::ZERO,
    });
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
