//! The floating toolbar pinned to the graph view's top-left corner: a run/cancel toggle and
//! an event-loop start/stop toggle side by side on one pill, with three view-framing buttons
//! (reset view, show all, show selected) stacked beneath on a second pill. The buttons are
//! opaque chips raised off the pill, so the toolbar stays legible over any node under it. The
//! toggles paint "toggled" while their action is in flight, and each button maps to an
//! [`AppCommand`].

use palantir::{Align, Configure, Panel, SizeSpec, Spacing, Ui, WidgetId};
use scenarium::GraphId;

use crate::gui::frame::app_command::AppCommand;
use crate::gui::frame::app_command::{RunCommand, ViewCommand};
use crate::gui::frame::framing::Framing;
use crate::gui::frame::requests::Requests;
use crate::gui::frame::shortcuts;
use crate::gui::frame::stage::Stage;
use crate::gui::graph_ctx::GraphCtx;
use crate::gui::widgets::chip::Chip;
use crate::gui::widgets::icon::Icon;
use crate::gui::widgets::pill;

/// The toolbar's chip ids, one set per graph pane.
fn run_button_wid(graph: GraphId) -> WidgetId {
    WidgetId::from_hash(("darkroom.graph.run_button", graph))
}

fn events_button_wid(graph: GraphId) -> WidgetId {
    WidgetId::from_hash(("darkroom.graph.events_button", graph))
}

fn reset_view_wid(graph: GraphId) -> WidgetId {
    WidgetId::from_hash(("darkroom.graph.reset_view_button", graph))
}

fn show_all_wid(graph: GraphId) -> WidgetId {
    WidgetId::from_hash(("darkroom.graph.show_all_button", graph))
}

fn show_selected_wid(graph: GraphId) -> WidgetId {
    WidgetId::from_hash(("darkroom.graph.show_selected_button", graph))
}

/// Draw the toolbar over the graph view's top-left corner. A click queues its
/// [`AppCommand`]. It hit-tests above the canvas (drawn after it), so a click on
/// a button never starts a pan.
pub(super) fn show(ui: &mut Ui, stage: &mut Stage<'_, GraphCtx<'_>>) {
    let (graph_ctx, out) = (stage.ctx, &mut *stage.out);
    let graph = graph_ctx.graph();
    let theme = graph_ctx.theme();
    let run_state = graph_ctx.run_state();
    Panel::vstack()
        .id_salt("graph_toolbar")
        .size(SizeSpec::HUG)
        .align(Align::TOP_LEFT)
        .child_align(Align::TOP_LEFT)
        .margin(Spacing::new(theme.space.sm, theme.space.sm, 0.0, 0.0))
        .gap(theme.space.sm)
        .show(ui, |ui| {
            // Both toggles compile and run the whole document.
            pill::dress(Panel::hstack().id_salt("graph_toolbar_run"), theme).show(ui, |ui| {
                let running = run_state.activity().is_executing();
                let run_tip = if running { "Cancel run" } else { "Run" };
                // Run alone idles with a status-colored glyph; the event-loop toggle sits muted
                // beside it like the framing buttons below.
                if Chip::new(run_button_wid(graph), run_tip)
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
                let event_loop_active = run_state.activity().event_loop_active();
                let events_tip = if event_loop_active {
                    "Stop events"
                } else {
                    "Start events"
                };
                if Chip::new(events_button_wid(graph), events_tip)
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

            // Each framing action takes the one route a framing has, which lands it in this
            // click's second pass.
            let framing = Panel::vstack()
                .id_salt("graph_toolbar_framing")
                .child_align(Align::TOP_LEFT);
            pill::dress(framing, theme).show(ui, |ui| {
                if Chip::new(reset_view_wid(graph), "Reset view")
                    .shortcut(shortcuts::RESET_VIEW)
                    .show_icon(ui, theme, Icon::ResetView)
                {
                    frame(out, graph, Framing::Reset);
                }
                if Chip::new(show_all_wid(graph), "Show all").show_icon(ui, theme, Icon::ShowAll) {
                    frame(out, graph, Framing::ShowAll);
                }
                if Chip::new(show_selected_wid(graph), "Show selected").show_icon(
                    ui,
                    theme,
                    Icon::ShowSelected,
                ) {
                    frame(out, graph, Framing::ShowSelected);
                }
            });
        });
}

fn frame(out: &mut Requests, graph: GraphId, framing: Framing) {
    out.push_app(AppCommand::View(ViewCommand::Frame { graph, framing }));
}

#[cfg(test)]
pub(crate) mod internals {
    use super::*;

    /// The run/cancel chip of `graph`'s pane, for the tests that click it.
    pub(crate) fn run_chip_wid(graph: GraphId) -> WidgetId {
        run_button_wid(graph)
    }

    /// The reset-view chip of `graph`'s pane, for the tests that click it.
    pub(crate) fn reset_view_chip_wid(graph: GraphId) -> WidgetId {
        reset_view_wid(graph)
    }
}
