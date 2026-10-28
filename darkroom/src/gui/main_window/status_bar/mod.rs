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

use palantir::{
    Align, Chrome, Configure, Panel, Sizing, Spacing, Text, TextStyle, Ui, VAlign, WidgetId, fmt,
};
use scenarium::RamUsage;

use crate::core::document::item_id::ItemId;
use crate::core::edit::graph_intent::GraphIntent;
use crate::core::status_log::Severity;
use crate::gui::frame::app_command::{AppCommand, ViewCommand};
use crate::gui::frame::requests::Requests;
use crate::gui::frame::window_ctx::WindowCtx;
use crate::gui::pane::graph_ui::gesture::pan_zoom::Framing;
use crate::gui::state::run_state::run_outcome::RunOutcome;
use crate::gui::theme::Theme;
use crate::gui::theme::type_scale;
use crate::gui::widgets::format::bytes::Bytes;
use crate::gui::widgets::format::percent::Percent;
use crate::gui::widgets::link::Link;
use crate::gui::widgets::support;
use std::collections::BTreeSet;
use std::fmt;
use std::fmt::Display;
use std::fmt::Formatter;

/// The bar is a window singleton, so it carries a global id rather than a
/// parent-scoped salt — one strip, one name, reachable from a test.
pub(crate) fn wid() -> WidgetId {
    WidgetId::from_hash("darkroom.status_bar")
}

/// Draw the bottom status bar. A link in the run line raises its edits on `out`.
pub(crate) fn show(ui: &mut Ui, ctx: WindowCtx<'_>, out: &mut Requests) {
    let app = ctx.app();
    let theme = app.theme();
    let ram = MemoryLabel::resolve(app.process_memory(), app.run_state().cache_ram());
    // A row's height, and its text at the inset of a menu title's on the band above.
    let inset = theme.surround_inset() + theme.space.md;
    Panel::hstack()
        .id(wid())
        .size((Sizing::FILL, Sizing::fixed(theme.row_height())))
        .child_align(Align::v(VAlign::Center))
        .padding(Spacing::xy(inset, 0.0))
        .gap(theme.space.sm)
        .chrome(Chrome::fill(theme.colors.surround_bg))
        .show(ui, |ui| {
            // A failed action outranks the run's tally: it is what just went wrong.
            if let Some(message) = app.status_message() {
                let color = match message.severity {
                    Severity::Error => theme.status.error,
                    Severity::Warning => theme.status.warning,
                };
                let style = support::colored_text(ui, color, theme.font.body);
                Text::new(message.line).style(&style).show(ui);
            } else if let Some(outcome) = app.run_state().outcome() {
                run_line(ui, ctx, outcome, out);
            }
            support::hspacer(ui, "status_spacer");
            let view = &ctx.document().main_view;
            let counts = CountsClause {
                node_count: ctx.document().graph.len(),
                selected_count: view.selected.len(),
            };
            Readout {
                counts,
                zoom: view.viewport.zoom,
                ram,
            }
            .show(ui, theme);
        });
}

/// The canvas zoom's width floor in figures: `Percent` writes the canvas's 10 % to 500 % in 3 or
/// 4.
const ZOOM_GLYPHS: u8 = 4;

/// A byte figure's width floor: `Bytes` writes at most 10 below 1000 GiB, as `1023.9 MiB`.
const BYTES_GLYPHS: u8 = 10;

/// The graph's counts and zoom, then the memory, as one line of clauses: `19 nodes · 1 selected
/// · 62% · Memory 3.0 MiB · Cache 1.0 KiB`.
#[derive(Clone, Copy, Debug)]
struct Readout {
    counts: CountsClause,
    zoom: f32,
    ram: Option<MemoryLabel>,
}

impl Readout {
    /// The figures that change while the line is watched draw in the figure face at the width of
    /// their widest value, so the words beside them hold still as a digit count changes. Each
    /// starts its box, beside the word that names it, and the box's slack trails it.
    fn show(self, ui: &mut Ui, theme: &Theme) {
        let Self { counts, zoom, ram } = self;
        let words = support::muted_text(ui, theme, theme.font.body);
        let figures = TextStyle {
            color: theme.ink.muted,
            ..support::mono_text(ui, theme.font.body)
        };
        let width = |glyphs| type_scale::figure_width(theme.font.body, glyphs);
        Panel::hstack()
            .id_salt("status_readout")
            .size((Sizing::HUG, Sizing::HUG))
            .child_align(Align::v(VAlign::Center))
            .show(ui, |ui| {
                Text::new(fmt!(ui, "{counts} \u{b7} "))
                    .style(&words)
                    .show(ui);
                Figure {
                    value: Percent::new(zoom),
                    min_width: width(ZOOM_GLYPHS),
                }
                .show(ui, &figures);
                let Some(MemoryLabel { process, cached }) = ram else {
                    return;
                };
                Text::new(" \u{b7} Memory ").style(&words).show(ui);
                Figure {
                    value: Bytes::new(process),
                    min_width: width(BYTES_GLYPHS),
                }
                .show(ui, &figures);
                if cached > 0 {
                    Text::new(" \u{b7} Cache ").style(&words).show(ui);
                    Figure {
                        value: Bytes::new(cached),
                        min_width: width(BYTES_GLYPHS),
                    }
                    .show(ui, &figures);
                }
            });
    }
}

/// A figure at the start of a box at least `min_width` wide.
#[derive(Clone, Copy, Debug)]
struct Figure<T> {
    value: T,
    min_width: f32,
}

impl<T: Display> Figure<T> {
    fn show(self, ui: &mut Ui, style: &TextStyle) {
        let Self { value, min_width } = self;
        let text = fmt!(ui, "{value}");
        Text::new(text)
            .style(style)
            .min_size((min_width, 0.0))
            .show(ui);
    }
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
        theme.ink.muted
    };
    let style = support::colored_text(ui, color, theme.font.body);
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
    let text = fmt!(ui, "Show {name}");
    if Link::new(cause_link_wid(), text).show(ui, theme) {
        out.push_graph(GraphIntent::SetSelection {
            to: BTreeSet::from([ItemId::Node(cause)]),
        });
        out.push_app(AppCommand::View(ViewCommand::Frame(Framing::ShowSelected)));
    }
}

fn cause_link_wid() -> WidgetId {
    WidgetId::from_hash("darkroom.status_bar.cause")
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

/// The graph's counts: how many nodes it holds and how many items are selected, as
/// `19 nodes · 1 selected`.
#[derive(Clone, Copy, Debug)]
struct CountsClause {
    node_count: usize,
    selected_count: usize,
}

impl Display for CountsClause {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let Self {
            node_count,
            selected_count,
        } = *self;
        write!(
            f,
            "{} \u{b7} {selected_count} selected",
            Counted(node_count, "node", "nodes")
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

#[cfg(test)]
mod tests;
