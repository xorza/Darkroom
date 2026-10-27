//! The window's bottom status bar: a thin surround strip carrying the last
//! failed action's message or a warning (left, in the error or the warning
//! color — run/save/load failures and what a load repaired, parked in the
//! engine's `StatusLog` until a success of the same family clears it), else
//! what the last run came to with a link to the node that failed; then, at
//! the right, the graph's node and selection counts and its zoom, and the
//! memory readout: this process's own resident footprint, plus the runtime
//! cache's system + GPU bytes under one `Cache` clause, mirrored from the last
//! completed `RunSummary` and shown only when the cache holds something.
//! The graph's line is always there, so the bar is one line tall on every
//! frame: a strip that grew and shrank would read as layout jitter and resize
//! the dock when a run lands.

use palantir::prelude::*;
use scenarium::RamUsage;

use crate::core::document::item_id::ItemId;
use crate::core::edit::graph_intent::GraphIntent;
use crate::core::status_log::Severity;
use crate::gui::frame::app_command::{AppCommand, ViewCommand};
use crate::gui::frame::requests::Requests;
use crate::gui::frame::window_ctx::WindowCtx;
use crate::gui::pane::graph_ui::gesture::pan_zoom::Framing;
use crate::gui::state::run_state::run_outcome::RunOutcome;
use crate::gui::widgets::format::bytes::Bytes;
use crate::gui::widgets::support;
use std::collections::BTreeSet;
use std::fmt;
use std::fmt::Display;
use std::fmt::Formatter;

/// The bar is a window singleton, so it carries a global id rather than a
/// parent-scoped salt — one strip, one name, reachable from a test.
pub(crate) fn id() -> WidgetId {
    WidgetId::from_hash("darkroom::status_bar")
}

/// Draw the bottom status bar. A link in the run line raises its edits on `out`.
pub(crate) fn show(ui: &mut Ui, ctx: WindowCtx<'_>, out: &mut Requests) {
    let app = ctx.app();
    let theme = app.theme();
    let ram = MemoryLabel::resolve(app.process_memory(), app.run_state().cache_ram());
    Panel::hstack()
        .id(id())
        .size((Sizing::FILL, Sizing::HUG))
        .child_align(Align::new(HAlign::Right, VAlign::Center))
        .padding(Spacing::xy(theme.space.md, theme.space.xs))
        .gap(theme.space.sm)
        .chrome(Chrome::fill(theme.colors.surround_fill))
        .show(ui, |ui| {
            // A failed action outranks the run's tally: it is what just went wrong.
            if let Some(message) = app.status_message() {
                let color = match message.severity {
                    Severity::Error => theme.status.error,
                    Severity::Warning => theme.status.warning,
                };
                let style = support::colored_text(ui, color, theme.text.body);
                Text::new(message.line).style(&style).show(ui);
            } else if let Some(outcome) = app.run_state().outcome() {
                run_line(ui, ctx, outcome, out);
            }
            support::hspacer(ui, "status_spacer");
            let view = &ctx.document().main_view;
            let canvas = CanvasLine {
                node_count: ctx.document().graph.len(),
                selected_count: view.selected.len(),
                zoom: view.viewport.zoom,
            };
            let style = support::muted_text(ui, theme, theme.text.body);
            Text::new(fmt!(ui, "{canvas}")).style(&style).show(ui);
            if let Some(label) = ram {
                let style = support::muted_text(ui, theme, theme.text.body);
                Text::new(fmt!(ui, "{label}")).style(&style).show(ui);
            }
        });
}

/// What the last run came to, in the ink of its worst case, and — when a node needs a look —
/// a link that selects it and frames it.
fn run_line(ui: &mut Ui, ctx: WindowCtx<'_>, outcome: RunOutcome, out: &mut Requests) {
    let theme = ctx.app().theme();
    let color = if outcome.failed_count > 0 {
        theme.status.error
    } else if outcome.missing_count > 0 {
        theme.status.warning
    } else {
        theme.colors.text_muted
    };
    let style = support::colored_text(ui, color, theme.text.body);
    Text::new(fmt!(ui, "{}", RunLine(outcome)))
        .style(&style)
        .show(ui);
    // The tally is a record of that run, so its cause may be gone from the graph since.
    let Some(cause) = outcome.cause else {
        return;
    };
    let Some(node) = ctx.document().graph.find(cause) else {
        return;
    };
    let name = &node.name;
    let wid = cause_link_wid();
    let link = if ui.response_for(wid).hovered() {
        theme.colors.link_hovered
    } else {
        theme.colors.link
    };
    let style = support::colored_text(ui, link, theme.text.body);
    if Text::new(fmt!(ui, "Show {name}"))
        .id(wid)
        .style(&style)
        .sense(Sense::CLICK)
        .show(ui)
        .left
        .clicked()
    {
        out.push_graph(GraphIntent::SetSelection {
            to: BTreeSet::from([ItemId::Node(cause)]),
        });
        out.push_app(AppCommand::View(ViewCommand::Frame(Framing::ShowSelected)));
    }
}

fn cause_link_wid() -> WidgetId {
    WidgetId::from_hash("darkroom::status_bar::cause")
}

/// The run line's text: the worst of the run first, then what else it counted.
#[derive(Clone, Copy, Debug)]
struct RunLine(RunOutcome);

impl Display for RunLine {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let RunOutcome {
            ran_count,
            cached_count,
            failed_count,
            skipped_count,
            missing_count,
            cancelled,
            ..
        } = self.0;
        if cancelled {
            return f.write_str("Run cancelled.");
        }
        if failed_count > 0 {
            write!(
                f,
                "Run failed: {}",
                Counted(failed_count, "error", "errors")
            )?;
            if skipped_count > 0 {
                write!(f, ", {skipped_count} skipped")?;
            }
            return f.write_str(".");
        }
        if missing_count > 0 {
            return write!(
                f,
                "Run incomplete: {} missing an input.",
                Counted(missing_count, "node", "nodes")
            );
        }
        if ran_count + cached_count == 0 {
            return f.write_str("Nothing ran.");
        }
        write!(f, "Ran {}", Counted(ran_count, "node", "nodes"))?;
        if cached_count > 0 {
            write!(f, ", {cached_count} cached")?;
        }
        f.write_str(".")
    }
}

/// The graph's line: how many nodes it holds, how many items are selected, and the camera's
/// zoom, as `19 nodes · 1 selected · 62 %`.
#[derive(Clone, Copy, Debug)]
struct CanvasLine {
    node_count: usize,
    selected_count: usize,
    zoom: f32,
}

impl Display for CanvasLine {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let Self {
            node_count,
            selected_count,
            zoom,
        } = *self;
        write!(
            f,
            "{} \u{b7} {selected_count} selected \u{b7} {:.0} %",
            Counted(node_count, "node", "nodes"),
            zoom * 100.0
        )
    }
}

/// `count` with its noun, singular for one.
#[derive(Clone, Copy, Debug)]
struct Counted(usize, &'static str, &'static str);

impl Display for Counted {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let Self(count, one, many) = *self;
        write!(f, "{count} {}", if count == 1 { one } else { many })
    }
}

#[derive(Clone, Copy, Debug)]
struct MemoryLabel {
    /// This process's resident footprint. Never zero — a zero one is what
    /// [`Self::resolve`] answers `None` to.
    process: u64,
    /// The runtime cache's two pools summed. Zero drops the clause.
    cached: u64,
}

impl MemoryLabel {
    /// The readout for one frame's figures, or `None` when no footprint is
    /// available — which leaves nothing worth a row.
    fn resolve(process: u64, cache: RamUsage) -> Option<Self> {
        (process != 0).then(|| Self {
            process,
            cached: cache.total() as u64,
        })
    }
}

impl Display for MemoryLabel {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "MEM {}", Bytes::new(self.process))?;
        if self.cached > 0 {
            write!(f, " · Cache {}", Bytes::new(self.cached))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
