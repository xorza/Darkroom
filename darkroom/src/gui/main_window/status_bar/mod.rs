//! The window's bottom status bar: at the left the last failed action's message or warning
//! (in the error or warning color: run/save/load failures and what a load repaired, parked in
//! the engine's `StatusLog` until a success of the same family clears it), else what the last
//! run came to with a link to the node that failed; at the right the graph's node and selection
//! counts, then the memory readout: this process's resident footprint, plus the
//! runtime cache's system + GPU bytes under one `Cache` clause, shown only when the cache holds
//! something. The graph's line is always there, so the bar is one line tall on every frame: a
//! strip that grew and shrank would jitter and resize the dock when a run lands.
//!
//! - **The memory figures stand in cells**, each right-aligned in a box of its widest value's
//!   width with a hairline before it, so no gap moves as a digit count changes.

use palantir::{
    Align, Chrome, Configure, Panel, PointerButton, Separator, SizeSpec, Sizing, Spacing, Text,
    TextStyle, Ui, VAlign, WidgetId, fmt,
};
use scenarium::{GraphId, RamUsage};

use crate::core::document::item_id::ItemId;
use crate::core::edit::graph_intent::GraphIntent;
use crate::core::status_log::Severity;
use crate::gui::frame::app_command::AppCommand;
use crate::gui::frame::doc_ctx::DocCtx;
use crate::gui::frame::framing::Framing;
use crate::gui::frame::pane_ask::PaneAsk;
use crate::gui::frame::stage::Stage;
use crate::gui::frame::window_parts::StatusInputs;
use crate::gui::pane::graph_ui::canvas;
use crate::gui::pane::graph_ui::canvas_input::{CanvasGesture, GestureHint};
use crate::gui::state::run_state::run_outcome::RunOutcome;
use crate::gui::theme::Theme;
use crate::gui::widgets::format::bytes::Bytes;
use crate::gui::widgets::link::Link;
use crate::gui::widgets::support;
use std::collections::BTreeSet;
use std::fmt;
use std::fmt::Display;
use std::fmt::Formatter;

/// A global id rather than a parent-scoped salt: the bar is a window singleton a test reaches.
pub(crate) fn wid() -> WidgetId {
    WidgetId::from_hash("darkroom.status_bar")
}

/// Draw the bottom status bar. A link in its run line raises its edits on the stage's requests.
pub(crate) fn show(ui: &mut Ui, stage: &mut Stage<'_, DocCtx<'_>>, status: StatusInputs<'_>) {
    let ctx = stage.ctx;
    let theme = ctx.theme();
    let ram = MemoryLabel::resolve(status.process_memory, ctx.run_state().cache_ram());
    // A row's height, and its text at the inset of a menu title's on the band above.
    let inset = theme.surround_inset() + theme.space.md;
    Panel::hstack()
        .id(wid())
        .size((Sizing::FILL, Sizing::fixed(theme.row_height())))
        .child_align(VAlign::Center)
        .padding(Spacing::xy(inset, 0.0))
        .gap(theme.space.sm)
        .chrome(Chrome::new(theme.colors.surround_bg))
        .show(ui, |ui| {
            // A failed action outranks the run's tally: it is what just went wrong.
            if let Some(message) = status.message {
                let color = match message.severity {
                    Severity::Error => theme.status.error,
                    Severity::Warning => theme.status.warning,
                };
                let style = support::colored_text(ui, color, theme.font.body);
                Text::new(message.line).style(&style).show(ui);
            } else if let Some(outcome) = ctx.run_state().outcome() {
                run_line(ui, stage, outcome);
            } else if ui.response_for(canvas::outer_wid(ctx.active())).hovered() {
                gesture_hints(ui, theme);
            }
            support::hspacer(ui, "status_spacer");
            let (document, active) = (ctx.document(), ctx.active());
            let view = document.view(active);
            let counts = CountsClause {
                node_count: document.graphs.graph(active).len(),
                selected_count: view.selected.len(),
            };
            Readout { counts, ram }.show(ui, theme);
        });
}

/// What the bare canvas under the pointer takes, in the words the reference shows: what a
/// newcomer reaches first, as Blender's status bar names what each button does where the pointer
/// is ([status bar](https://docs.blender.org/manual/en/latest/interface/window_system/status_bar.html)).
fn gesture_hints(ui: &mut Ui, theme: &Theme) {
    let style = support::muted_text(ui, theme, theme.font.body);
    let line = fmt!(
        ui,
        "{}",
        GestureLine(&[
            CanvasGesture::NewNode,
            CanvasGesture::Select,
            CanvasGesture::Pan,
            CanvasGesture::Breaker(PointerButton::Right),
        ])
    );
    Text::new(line).style(&style).show(ui);
}

/// Each gesture's keys and what it does, `double-click Add a node · drag Select with a band`.
#[derive(Clone, Copy, Debug)]
struct GestureLine<'a>(&'a [CanvasGesture]);

impl Display for GestureLine<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        for (idx, gesture) in self.0.iter().enumerate() {
            if idx > 0 {
                f.write_str(" \u{b7} ")?;
            }
            let GestureHint { keys, does } = gesture.hint();
            write!(f, "{keys} {does}")?;
        }
        Ok(())
    }
}

/// A byte figure's width floor: `Bytes` writes at most 10 below 1000 GiB, as `1023.9 MiB`.
const BYTES_GLYPHS: u8 = 10;

/// The graph's counts, then the memory in cells: `19 nodes · 1 selected │ Memory 3.0 MiB │
/// Cache 1.0 KiB`. The zoom is the pane band's control.
#[derive(Clone, Copy, Debug)]
struct Readout {
    counts: CountsClause,
    ram: Option<MemoryLabel>,
}

impl Readout {
    fn show(self, ui: &mut Ui, theme: &Theme) {
        let Self { counts, ram } = self;
        let styles = CellStyles {
            words: support::muted_text(ui, theme, theme.font.body),
            figures: TextStyle {
                color: theme.ink.muted,
                ..support::mono_text(ui, theme.font.body)
            },
        };
        Panel::hstack()
            .id_salt("status_readout")
            .size(SizeSpec::HUG)
            .child_align(VAlign::Center)
            .show(ui, |ui| {
                Text::new(fmt!(ui, "{counts}"))
                    .style(&styles.words)
                    .show(ui);
                let Some(MemoryLabel { process, cached }) = ram else {
                    return;
                };
                Cell {
                    word: "Memory",
                    value: Bytes::new(process),
                    glyphs: BYTES_GLYPHS,
                }
                .show(ui, theme, &styles);
                if cached > 0 {
                    Cell {
                        word: "Cache",
                        value: Bytes::new(cached),
                        glyphs: BYTES_GLYPHS,
                    }
                    .show(ui, theme, &styles);
                }
            });
    }
}

/// The two inks of the readout: its words, and its figures in the figure face.
#[derive(Debug)]
struct CellStyles {
    words: TextStyle,
    figures: TextStyle,
}

/// One cell of the readout: a hairline before it, its word, then its figure right-aligned in a
/// box as wide as `glyphs` figures, so neither the cell nor any hairline after it moves as the
/// figure's digit count changes, and figures line up on their units.
#[derive(Clone, Copy, Debug)]
struct Cell<T> {
    word: &'static str,
    value: T,
    glyphs: u8,
}

impl<T: Display> Cell<T> {
    fn show(self, ui: &mut Ui, theme: &Theme, styles: &CellStyles) {
        let Self {
            word,
            value,
            glyphs,
        } = self;
        let width = support::figure_width(ui, &styles.figures, glyphs);
        Panel::hstack()
            .id(cell_wid(word))
            .size(SizeSpec::HUG)
            .padding(Spacing::new(theme.space.md, 0.0, 0.0, 0.0))
            .gap(theme.space.sm)
            .child_align(VAlign::Center)
            .show(ui, |ui| {
                Separator::vertical()
                    .size((
                        Sizing::fixed(theme.stroke.hairline),
                        Sizing::fixed(theme.font.body),
                    ))
                    .margin(Spacing::new(0.0, 0.0, theme.space.sm, 0.0))
                    .show(ui);
                Text::new(word).style(&styles.words).show(ui);
                Text::new(fmt!(ui, "{value}"))
                    .style(&styles.figures)
                    .size((Sizing::fixed(width), Sizing::HUG))
                    .text_align(Align::RIGHT)
                    .show(ui);
            });
    }
}

/// The readout cell `word` names.
fn cell_wid(word: &'static str) -> WidgetId {
    WidgetId::from_hash(("darkroom.status_bar.cell", word))
}

/// What the last run came to, in the ink of its worst case, and — when a node needs a look —
/// a link that selects it and frames it.
fn run_line(ui: &mut Ui, stage: &mut Stage<'_, DocCtx<'_>>, outcome: RunOutcome) {
    let ctx = stage.ctx;
    let theme = ctx.theme();
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
    // The tally is a record of that run, so its cause may be gone from the graph since. The link
    // shows the cause in its own graph's pane, entered through the instance it ran in.
    let Some(cause) = outcome.cause else {
        return;
    };
    let graphs = &ctx.document().graphs;
    let Some(graph) = graphs.graph_at(cause.path()) else {
        return;
    };
    let Some(node) = graphs.graph(graph).find(cause.node_id()) else {
        return;
    };
    let name = graphs.scope(ctx.library(), graph).title(node);
    let text = fmt!(ui, "Show {name}");
    if Link::new(cause_link_wid(), text).show(ui, theme) {
        if let GraphId::Compound(compound) = graph {
            stage.out.push_enter(compound, Some(cause.path()));
        }
        stage.out.push_graph(
            graph,
            GraphIntent::SetSelection {
                to: BTreeSet::from([ItemId::Node(cause.node_id())]),
            },
        );
        stage.out.push_app(AppCommand::Pane {
            graph,
            ask: PaneAsk::Frame(Framing::Selection),
        });
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
