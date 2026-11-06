//! The floating toolbar pinned to the graph view's top-left corner: a
//! run/cancel toggle and an event-loop start/stop toggle side by side on one
//! pill, with three view-framing buttons (reset view, show all, show
//! selected) stacked beneath on a second pill.
//! The buttons are opaque chips raised off the pill, so the toolbar stays
//! legible over both the canvas and any node under it. All carry
//! hover tooltips; the toggles paint "toggled" while their action is in flight,
//! and each button maps to an [`AppCommand`].

use palantir::{Align, Configure, Panel, Sizing, Spacing, Ui, WidgetId};

use crate::gui::frame::app_command::AppCommand;
use crate::gui::frame::app_command::{RunCommand, ViewCommand};
use crate::gui::frame::framing::Framing;
use crate::gui::frame::record_stage::RecordStage;
use crate::gui::frame::requests::Requests;
use crate::gui::frame::shortcuts;
use crate::gui::graph_ctx::GraphCtx;
use crate::gui::widgets::chip::Chip;
use crate::gui::widgets::icon::Icon;
use crate::gui::widgets::pill;

/// The toolbar's chip ids. One graph pane, so each is a fixed hash.
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

/// Draw the toolbar over the graph view's top-left corner. A click queues its
/// [`AppCommand`]. It hit-tests above the canvas (drawn after it), so a click on
/// a button never starts a pan.
pub(super) fn show(ui: &mut Ui, stage: &mut RecordStage<'_, GraphCtx<'_>>) {
    let (graph_ctx, out) = (stage.ctx, &mut *stage.out);
    let theme = graph_ctx.theme();
    let run_state = graph_ctx.run_state();
    Panel::vstack()
        .id_salt("graph_toolbar")
        .size((Sizing::HUG, Sizing::HUG))
        .align(Align::TOP_LEFT)
        .child_align(Align::TOP_LEFT)
        .margin(Spacing::new(theme.space.sm, theme.space.sm, 0.0, 0.0))
        .gap(theme.space.sm)
        .show(ui, |ui| {
            // Top row: run/cancel + event-loop toggles, side by side on their
            // own pill. Both compile and run the whole document.
            pill::dress(Panel::hstack().id_salt("graph_toolbar_run"), theme).show(ui, |ui| {
                // Run / cancel: toggled while a one-shot run is in
                // flight.
                let running = run_state.activity().is_executing();
                let run_tip = if running { "Cancel run" } else { "Run" };
                // Run is the one primary action in the cluster — it
                // alone idles with the accent glyph; the event-loop
                // toggle sits muted beside it like the framing
                // buttons below.
                if Chip::new(run_button_wid(), run_tip)
                    .shortcut(shortcuts::RUN)
                    .selected(running)
                    .idle_glyph(theme.status.success)
                    .ink(theme.status.busy)
                    .show_icon(ui, theme, Icon::Run)
                {
                    out.push_app(AppCommand::Run(if running {
                        RunCommand::Cancel
                    } else {
                        RunCommand::Once
                    }));
                }
                // Event loop start / stop: toggled while the loop runs.
                let event_loop_active = run_state.activity().event_loop_active();
                let events_tip = if event_loop_active {
                    "Stop events"
                } else {
                    "Start events"
                };
                if Chip::new(events_button_wid(), events_tip)
                    .selected(event_loop_active)
                    .ink(theme.status.busy)
                    .show_icon(ui, theme, Icon::RunEvents)
                {
                    out.push_app(AppCommand::Run(if event_loop_active {
                        RunCommand::StopEvents
                    } else {
                        RunCommand::StartEvents
                    }));
                }
            });

            // View-framing actions, stacked under the run row on their own
            // pill. Each takes the one route a framing has, which lands it in
            // this click's second pass.
            let framing = Panel::vstack()
                .id_salt("graph_toolbar_framing")
                .child_align(Align::TOP_LEFT);
            pill::dress(framing, theme).show(ui, |ui| {
                if Chip::new(reset_view_wid(), "Reset view")
                    .shortcut(shortcuts::RESET_VIEW)
                    .show_icon(ui, theme, Icon::ResetView)
                {
                    frame(out, Framing::Reset);
                }
                if Chip::new(show_all_wid(), "Show all").show_icon(ui, theme, Icon::ShowAll) {
                    frame(out, Framing::ShowAll);
                }
                if Chip::new(show_selected_wid(), "Show selected").show_icon(
                    ui,
                    theme,
                    Icon::ShowSelected,
                ) {
                    frame(out, Framing::ShowSelected);
                }
            });
        });
}

fn frame(out: &mut Requests, framing: Framing) {
    out.push_app(AppCommand::View(ViewCommand::Frame(framing)));
}

#[cfg(test)]
pub(crate) mod internals {
    use super::*;

    /// The run/cancel chip, for the tests that click it.
    pub(crate) fn run_chip_wid() -> WidgetId {
        run_button_wid()
    }

    /// The reset-view chip, for the tests that click it.
    pub(crate) fn reset_view_chip_wid() -> WidgetId {
        reset_view_wid()
    }
}
