//! The floating toolbar pinned to the graph view's top-left corner: a
//! run/cancel toggle and an event-loop start/stop toggle side by side on one
//! chrome pill — drawn only on the main graph's pane, since both act on the
//! whole document — with three view-framing buttons (reset view, show all,
//! show selected) stacked beneath on a second pill that every pane carries.
//! The frosted pills keep the toolbar legible over both the canvas and any
//! node under it; the buttons are opaque chips raised off the pill. All carry
//! hover tooltips; the toggles paint "toggled" while their action is in flight
//! and map to an [`AppCommand`], while the framing buttons emit an
//! `GraphIntent::SetViewport` directly.

use glam::Vec2;
use palantir::{
    Align, RgbaF32, Configure, HAlign, Panel, Rect, Shape, Sizing, Spacing, Ui, VAlign, WidgetId,
};

use crate::gui::app::commands::AppCommand;
use crate::gui::app::commands::run::RunCommand;
use crate::gui::pane::graph::ctx::CanvasCtx;
use crate::gui::pane::graph::gesture::pan_zoom::{self, Framing};
use crate::gui::requests::Requests;
use crate::gui::widgets::support::{dot, filled_rect, frame, play_triangle, stroked_rect};
use crate::gui::widgets::toolbar::{BUTTON_GAP, Chip, TOOLBAR_MARGIN, pill};

/// The toolbar's chip ids. One graph pane, so each is a fixed hash rather than
/// keyed by the pane it sits on.
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

/// Draw the toolbar over the graph view's top-left corner. A run/events click
/// queues its [`AppCommand`]; a view-framing click queues a
/// `GraphIntent::SetViewport`. It hit-tests above the canvas (drawn after it),
/// so a click on a button never starts a pan.
pub(super) fn show(ui: &mut Ui, cx: CanvasCtx<'_>, out: &mut Requests) {
    let (theme, graph_ctx, geometry) = (cx.theme(), cx.graph_ctx(), cx.geometry());
    let run_state = graph_ctx.run_state();
    Panel::vstack()
        .id_salt("graph_toolbar")
        .size((Sizing::HUG, Sizing::HUG))
        .align(Align::new(HAlign::Left, VAlign::Top))
        .child_align(Align::new(HAlign::Left, VAlign::Top))
        .margin(Spacing::new(TOOLBAR_MARGIN, TOOLBAR_MARGIN, 0.0, 0.0))
        .gap(BUTTON_GAP)
        .show(ui, |ui| {
            // Top row: run/cancel + event-loop toggles, side by side on their
            // own chrome pill. Both compile and run the whole document.
            pill(
                ui,
                theme,
                Panel::hstack().id_salt("graph_toolbar_run"),
                |ui| {
                    // Run / cancel: toggled while a one-shot run is in
                    // flight.
                    let running = run_state.activity.is_executing();
                    let run_tip = if running { "Cancel run" } else { "Run" };
                    // Run is the one primary action in the cluster — it
                    // alone idles with the accent glyph; the event-loop
                    // toggle sits muted beside it like the framing
                    // buttons below.
                    if Chip::new(run_button_wid(), run_tip)
                        .toggled(running)
                        .idle_glyph(theme.status.success)
                        .toggled_fill(theme.status.busy)
                        .show(ui, theme, draw_play)
                    {
                        out.push_app(AppCommand::Run(if running {
                            RunCommand::Cancel
                        } else {
                            RunCommand::Once
                        }));
                    }
                    // Event loop start / stop: toggled while the loop runs.
                    let event_loop_active = run_state.activity.event_loop_active();
                    let events_tip = if event_loop_active {
                        "Stop events"
                    } else {
                        "Start events"
                    };
                    if Chip::new(events_button_wid(), events_tip)
                        .toggled(event_loop_active)
                        .toggled_fill(theme.status.busy)
                        .show(ui, theme, draw_play_bar)
                    {
                        out.push_app(AppCommand::Run(if event_loop_active {
                            RunCommand::StopEvents
                        } else {
                            RunCommand::StartEvents
                        }));
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
            pill(ui, theme, framing, |ui| {
                if Chip::new(reset_view_wid(), "Reset view").show(ui, theme, draw_reset) {
                    out.extend_graph(pan_zoom::framing_intent(
                        ui,
                        geometry,
                        graph_ctx,
                        Framing::Reset,
                    ));
                }
                if Chip::new(show_all_wid(), "Show all").show(ui, theme, draw_show_all) {
                    out.extend_graph(pan_zoom::framing_intent(
                        ui,
                        geometry,
                        graph_ctx,
                        Framing::ShowAll,
                    ));
                }
                if Chip::new(show_selected_wid(), "Show selected").show(
                    ui,
                    theme,
                    draw_show_selected,
                ) {
                    out.extend_graph(pan_zoom::framing_intent(
                        ui,
                        geometry,
                        graph_ctx,
                        Framing::ShowSelected,
                    ));
                }
            });
        });
}

/// A right-pointing play triangle (run once), optically centered in the box.
fn draw_play(ui: &mut Ui, s: f32, color: RgbaF32) {
    play_triangle(ui, s, PLAY_FILL, color);
}

/// `|>` — a vertical bar then a play triangle (start the event loop).
fn draw_play_bar(ui: &mut Ui, s: f32, color: RgbaF32) {
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

/// How much of a toolbar button the play mark spans. Smaller than the node
/// header's share of its badge: a 30px button carries proportionally less
/// glyph than an 18px chip.
const PLAY_FILL: f32 = 0.4;

/// Reset view: a target ring with a center dot (recenter to 1:1).
fn draw_reset(ui: &mut Ui, s: f32, color: RgbaF32) {
    let d = s * 0.52;
    let o = (s - d) * 0.5;
    stroked_rect(ui, Rect::new(o, o, d, d), d * 0.5, color, s * 0.06);
    dot(ui, s * 0.5, s * 0.5, s * 0.075, color);
}

/// Show all: a frame enclosing a 2×2 field of dots (fit every node).
fn draw_show_all(ui: &mut Ui, s: f32, color: RgbaF32) {
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
fn draw_show_selected(ui: &mut Ui, s: f32, color: RgbaF32) {
    frame(ui, s, color);
    let inner = s * 0.24;
    let o = (s - inner) * 0.5;
    filled_rect(ui, Rect::new(o, o, inner, inner), s * 0.04, color);
}

#[cfg(test)]
pub(crate) mod internals {
    use super::*;

    /// The run/cancel chip, for the tests that click it.
    pub(crate) fn run_chip_wid() -> WidgetId {
        run_button_wid()
    }
}
