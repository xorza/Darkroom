//! The node body's footer: one strip under the ports with the last run's status at the left and
//! the RAM and VRAM the node's cached output holds at the right, over a bar of their split; a
//! node that failed says why on a line above. Recorded only when there is something to say — a
//! status or bytes — so an idle node shows no strip.

use palantir::TextWrap;
use palantir::prelude::*;
use scenarium::{NodeId, RamUsage};

use crate::gui::graph_ctx::node_ctx::NodeCtx;
use crate::gui::pane::graph_ui::node_ui::card;
use crate::gui::pane::graph_ui::node_ui::card::LabeledValue;
use crate::gui::pane::graph_ui::wid;
use crate::gui::state::run_state::ExecStatus;
use crate::gui::theme::Theme;
use crate::gui::widgets::format::bytes::Bytes;
use crate::gui::widgets::format::elapsed::Elapsed;
use crate::gui::widgets::icon::Icon;
use crate::gui::widgets::support;

const DOT: f32 = 6.0;
const BAR_H: f32 = 3.0;

/// Width floor for the run-time label, ~7 mono glyphs at the label tier. The
/// range this covers is pinned by `every_run_time_fits_the_reserved_width`.
///
/// The node is `Hug` above a min width, so anything that changes the
/// footer's measured width moves the node's right edge — and with it the
/// cached intra-node offsets of every *output* port, which is what wires
/// anchor to. A live timer re-formats every frame, so without a floor a
/// running node twitches its outgoing wires each time the digit count
/// changes (`9.99s` → `10.00s`, `999.9ms` → `1.00s`). That is not an
/// `UndoStep`, so nothing requests the settle pass that would hide it.
///
/// A floor rather than a fixed width: every value up to `999999s` fits
/// inside it and measures identically, and the rare longer one still
/// renders rather than clipping. Generous on purpose — costing a few px
/// of footer is cheaper than being one glyph short of the common case.
const RUN_TIME_MIN_WIDTH: f32 = 52.0;

/// The run-time label's style: label-tier mono, in the status color.
fn run_time_style(ui: &Ui, theme: &Theme, color: RgbaF32) -> TextStyle {
    TextStyle {
        color,
        ..support::mono_text(ui, theme.text.label)
    }
}

/// Draw the node's footer, or nothing when the last run said nothing about it and it holds no
/// bytes. `node.ram()` is mirrored from the run cache; each pool shows only when non-zero.
pub(super) fn show(ui: &mut Ui, node_ctx: NodeCtx<'_>) {
    let node = node_ctx;
    let ram = node.ram();
    let status = node.exec_status();
    if *status == ExecStatus::None && ram.total() == 0 {
        return;
    }
    let theme = node_ctx.theme();
    // Same inner radius the card's stroke leaves (`CardTheme::inner_radius`),
    // not the card's raw outer `corner_radius` — else this strip's
    // corner leaves a wedge of body fill showing past the border stroke.
    let r = theme.card.inner_radius();

    Panel::vstack()
        .id(footer_wid(node.node_id))
        .size((Sizing::FILL, Sizing::HUG))
        .padding(card::band_padding(theme))
        .gap(theme.space.xs)
        // Round only the bottom corners so the strip seats into the node's
        // rounded bottom.
        .chrome(card::footer_chrome(theme, r))
        .show(ui, |ui| {
            if let ExecStatus::Errored(Some(message)) = status {
                error_line(ui, theme, message);
            }
            Panel::hstack()
                .id_salt("node_footer_row")
                .size((Sizing::FILL, Sizing::HUG))
                .gap(theme.space.md)
                .child_align(Align::v(VAlign::Center))
                .show(ui, |ui| {
                    run_status(ui, node_ctx);
                    support::hspacer(ui, "node_footer_spacer");
                    for pool in Pool::held(theme, ram) {
                        pool.meter(ui, theme);
                    }
                });
            if ram.total() > 0 {
                proportion_bar(ui, theme, ram);
            }
        });
}

/// The failure's first line in the error ink, cut to the card's width with the whole message
/// as its tooltip: the node says why it failed without growing to the message's length.
fn error_line(ui: &mut Ui, theme: &Theme, message: &str) {
    let first = message.lines().next().unwrap_or(message);
    let style = support::colored_text(ui, theme.status.error, theme.text.label);
    let line = Text::new(first)
        .style(&style)
        .text_wrap(TextWrap::Ellipsis)
        .size((Sizing::FILL, Sizing::HUG))
        .sense(Sense::HOVER)
        .show(ui)
        .snapshot();
    support::tooltip_after(ui, &line, Some(message));
}

/// The last run's word on the node, in its status ink: a check and the time when it ran, the
/// spinner and the live time while it runs, and a word for the rest.
fn run_status(ui: &mut Ui, node: NodeCtx<'_>) {
    let theme = node.theme();
    let status = node.exec_status();
    let color = theme.exec_color(status).unwrap_or(theme.colors.text_muted);
    match status {
        ExecStatus::None => {}
        ExecStatus::Executed(secs) => {
            let side = theme.text.label;
            Panel::zstack()
                .id_salt("node_footer_check")
                .size((Sizing::fixed(side), Sizing::fixed(side)))
                .show(ui, |ui| {
                    Icon::Check.draw(ui, Rect::new(0.0, 0.0, side, side), color);
                });
            run_time(ui, theme, *secs, color);
        }
        ExecStatus::Running(at) => {
            Spinner::new()
                .diameter(theme.text.label)
                .color(color)
                .show(ui);
            run_time(ui, theme, at.elapsed().as_secs_f64(), color);
        }
        ExecStatus::Cached => word(ui, theme, "Cached", color),
        ExecStatus::MissingInputs(_) => word(ui, theme, "Missing input", color),
        ExecStatus::Errored(_) => word(ui, theme, "Failed", color),
        ExecStatus::Skipped => word(ui, theme, "Skipped \u{b7} input failed", color),
    }
}

fn run_time(ui: &mut Ui, theme: &Theme, secs: f64, color: RgbaF32) {
    let elapsed = fmt!(ui, "{}", Elapsed::new(secs));
    Text::new(elapsed)
        .style(&run_time_style(ui, theme, color))
        .min_size((RUN_TIME_MIN_WIDTH, 0.0))
        .show(ui);
}

fn word(ui: &mut Ui, theme: &Theme, text: &'static str, color: RgbaF32) {
    let style = support::colored_text(ui, color, theme.text.label);
    Text::new(text).style(&style).show(ui);
}

/// One memory pool the node holds bytes in, and the hue it shows in.
#[derive(Debug, Clone, Copy)]
struct Pool {
    label: &'static str,
    hue: RgbaF32,
    bytes: usize,
}

impl Pool {
    /// The pools of `ram` that hold bytes: system RAM in the cache badge's hue, as a cache is what
    /// holds it, and GPU memory in the accent the graph badge wears.
    fn held(theme: &Theme, ram: RamUsage) -> impl Iterator<Item = Self> {
        [
            Self {
                label: "RAM",
                hue: theme.colors.badge_cache,
                bytes: ram.cpu,
            },
            Self {
                label: "VRAM",
                hue: theme.colors.badge_graph,
                bytes: ram.gpu,
            },
        ]
        .into_iter()
        .filter(|pool| pool.bytes > 0)
    }

    /// A colored dot, an uppercase micro-label, and the byte figure. The value takes the node's
    /// default text color; the label is muted.
    fn meter(self, ui: &mut Ui, theme: &Theme) {
        Panel::hstack()
            .id_salt(("node_mem_meter", self.label))
            .size((Sizing::HUG, Sizing::HUG))
            .gap(theme.space.xs)
            .child_align(Align::v(VAlign::Center))
            .show(ui, |ui| {
                dot_cell(ui, self.hue);
                LabeledValue {
                    label: self.label,
                    value: Bytes::new(self.bytes as u64),
                }
                .show(ui, theme);
            });
    }

    /// The pool's segment of the proportion bar, weighted by its bytes.
    fn bar_segment(self, ui: &mut Ui) {
        Panel::zstack()
            .id_salt(("node_mem_seg", self.label))
            .size((Sizing::fill(self.bytes as f32), Sizing::FILL))
            .chrome(Chrome::rounded(self.hue, Corners::all(BAR_H * 0.5)))
            .show(ui, |_ui| {});
    }
}

/// A small filled circle occupying `DOT`×`DOT` of layout so the label flows
/// after it. Named apart from `widgets::support::dot`, which emits a bare
/// `Shape` and takes no layout space — same picture, opposite contract.
fn dot_cell(ui: &mut Ui, hue: RgbaF32) {
    Panel::zstack()
        .id_salt("node_mem_dot")
        .size((Sizing::fixed(DOT), Sizing::fixed(DOT)))
        .chrome(Chrome::rounded(hue, Corners::all(DOT * 0.5)))
        .show(ui, |_ui| {});
}

/// A thin two-tone bar encoding the CPU:GPU split — each segment's width is its
/// byte share (weighted `Fill`). A single-pool footer reads as one full-width
/// color.
fn proportion_bar(ui: &mut Ui, theme: &Theme, ram: RamUsage) {
    Panel::hstack()
        .id_salt("node_mem_bar")
        .size((Sizing::FILL, Sizing::fixed(BAR_H)))
        .show(ui, |ui| {
            for pool in Pool::held(theme, ram) {
                pool.bar_segment(ui);
            }
        });
}

fn footer_wid(node_id: NodeId) -> WidgetId {
    wid::of_node("footer", node_id)
}

#[cfg(test)]
mod tests {
    use glam::UVec2;
    use palantir::internals::ui_harness::UiHarness;
    use palantir::{Configure, RgbaF32, Text, WidgetId};

    use crate::core::document::internals::DocumentFixture;
    use crate::gui::pane::graph_ui::internals::CanvasHarness;
    use crate::gui::pane::graph_ui::node_ui::node_footer::{
        RUN_TIME_MIN_WIDTH, footer_wid, run_time_style,
    };
    use crate::gui::pane::graph_ui::wid;
    use crate::gui::theme::Theme;
    use crate::gui::widgets::format::elapsed::Elapsed;

    /// Every run time `Elapsed::new` writes, from each unit's smallest to its
    /// widest, fits the floor the label reserves — measured with real shaping
    /// in the label's own style, so neither side can drift alone.
    #[test]
    fn every_run_time_fits_the_reserved_width() {
        let secs = [
            0.0, 999.4e-6, 999.94e-3, 9.994, 99.999, 999.994, 999.996, 9_999.96, 999_999.0,
        ];
        let theme = Theme::default();
        let mut h = UiHarness::with_text(UVec2::new(800, 200));
        let id = |i: usize| WidgetId::from_hash(("run_time", i));
        h.frame(|ui| {
            for (i, &s) in secs.iter().enumerate() {
                let style = run_time_style(ui, &theme, RgbaF32::WHITE);
                Text::new(Elapsed::new(s).to_string())
                    .id(id(i))
                    .style(&style)
                    .show(ui);
            }
        });
        for (i, &s) in secs.iter().enumerate() {
            let width = h.rect(id(i)).expect("label arranged").size.w;
            assert!(
                width <= RUN_TIME_MIN_WIDTH,
                "{} is {width} px wide, past the {RUN_TIME_MIN_WIDTH} px floor",
                Elapsed::new(s),
            );
        }
    }

    /// A node that never ran and holds no bytes has nothing to report, so it records no footer.
    #[test]
    fn an_idle_node_records_no_footer() {
        let mut h = CanvasHarness::new(DocumentFixture::probes(1));
        let node_id = h.node(0);
        h.prime(2);
        assert!(h.ui.rect(footer_wid(node_id)).is_none());
        assert!(h.ui.rect(wid::body(node_id)).is_some());
    }
}
