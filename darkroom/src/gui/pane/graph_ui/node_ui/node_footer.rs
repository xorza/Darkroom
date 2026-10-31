//! The node body's footer: one strip under the ports with the last run's status at the left and
//! the RAM and VRAM the node's cached output holds at the right, over a bar of their split; a
//! node that failed says why beside its cross. Recorded on every node, "Not run" before its first
//! run, so a run's end changes no node's height.

use palantir::{
    Align, Block, Chrome, Configure, Corners, Panel, RgbaF32, Sense, Sizing, Text, TextWrap, Ui,
    VAlign,
};
use scenarium::RamUsage;

use crate::gui::graph_ctx::node_ctx::NodeCtx;
use crate::gui::pane::graph_ui::node_ui::card;
use crate::gui::pane::graph_ui::node_ui::card::LabeledValue;
use crate::gui::pane::graph_ui::node_ui::run_status::RunStatus;
use crate::gui::pane::graph_ui::wid;
use crate::gui::state::run_state::ExecStatus;
use crate::gui::theme::Theme;
use crate::gui::widgets::format::bytes::Bytes;
use crate::gui::widgets::support;

const BAR_HEIGHT: f32 = 3.0;

/// Draw the node's footer. `node.ram()` is mirrored from the run cache; each pool shows only
/// when non-zero.
pub(super) fn show(ui: &mut Ui, node_ctx: NodeCtx<'_>) {
    let node = node_ctx;
    let ram = node.ram();
    let status = node.exec_status();
    let theme = node_ctx.theme();
    Panel::vstack()
        .id(wid::footer(node.node_id))
        .size((Sizing::FILL, Sizing::HUG))
        .padding(card::band_padding(theme))
        .gap(theme.space.xs)
        .chrome(card::footer_chrome(theme))
        .show(ui, |ui| {
            Panel::hstack()
                .id_salt("node_footer_row")
                .size((Sizing::FILL, Sizing::HUG))
                .gap(theme.space.md)
                .child_align(Align::v(VAlign::Center))
                .show(ui, |ui| {
                    let run = RunStatus::new(status);
                    match status {
                        ExecStatus::Errored(Some(_)) => run.failure_explained().show(ui, theme),
                        _ => run.show(ui, theme),
                    }
                    // A failure's message takes the row's free width, where the spacer would.
                    match status {
                        ExecStatus::Errored(Some(message)) => error_line(ui, theme, message),
                        _ => support::hspacer(ui, "node_footer_spacer"),
                    }
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
    let style = support::colored_text(ui, theme.status.error, theme.font.caption);
    let line = Text::new(first)
        .style(&style)
        .text_wrap(TextWrap::Ellipsis)
        .size((Sizing::FILL, Sizing::HUG))
        .sense(Sense::HOVER)
        .show(ui)
        .snapshot();
    support::tooltip_after(ui, &line, Some(message));
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
                hue: theme.badge.cache,
                bytes: ram.cpu,
            },
            Self {
                label: "VRAM",
                hue: theme.colors.accent,
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
                support::dot_cell(ui, "node_mem_dot", theme, self.hue);
                LabeledValue {
                    label: self.label,
                    value: Bytes::new(self.bytes as u64),
                }
                .show(ui, theme);
            });
    }

    /// The pool's segment of the proportion bar, weighted by its bytes.
    fn bar_segment(self, ui: &mut Ui) {
        Block::new()
            .id_salt(("node_mem_seg", self.label))
            .size((Sizing::fill(self.bytes as f32), Sizing::FILL))
            .chrome(Chrome::rounded(self.hue, Corners::all(BAR_HEIGHT * 0.5)))
            .show(ui);
    }
}

/// A thin two-tone bar encoding the CPU:GPU split — each segment's width is its
/// byte share (weighted `Fill`). A single-pool footer reads as one full-width
/// color.
fn proportion_bar(ui: &mut Ui, theme: &Theme, ram: RamUsage) {
    Panel::hstack()
        .id_salt("node_mem_bar")
        .size((Sizing::FILL, Sizing::fixed(BAR_HEIGHT)))
        .show(ui, |ui| {
            for pool in Pool::held(theme, ram) {
                pool.bar_segment(ui);
            }
        });
}

#[cfg(test)]
mod tests {

    use crate::core::document::internals::DocumentFixture;
    use crate::gui::pane::graph_ui::internals::CanvasHarness;
    use crate::gui::pane::graph_ui::wid;
    use crate::gui::theme::Theme;

    /// A failed node's footer is one row: the cross and the message's first line, the whole
    /// message its tooltip, one label line tall inside the band's padding, with no "Failed"
    /// beside a message that says why.
    #[test]
    fn a_failed_node_says_why_on_one_row() {
        use std::io;
        use std::sync::Arc;

        use scenarium::{InvokeError, NodeExecStatus, NodeStatus, RamUsage, RunError, RunSummary};

        use crate::gui::pane::graph_ui::node_ui::card;

        let mut h = CanvasHarness::new(DocumentFixture::probes(1));
        let node_id = h.node(0);
        h.ctx.run_state.complete(RunSummary {
            nodes: vec![NodeStatus {
                node_id,
                status: Some(NodeExecStatus::Errored {
                    error: RunError::Invoke(Arc::new(InvokeError::external(io::Error::other(
                        "no light frames\nprovided",
                    )))),
                }),
                ram: RamUsage::default(),
            }],
            ..RunSummary::default()
        });
        h.prime(2);
        let footer =
            h.ui.rect(wid::footer(node_id))
                .expect("a failure has a footer");
        let theme = Theme::default();
        // One label line, 12 × 1.2 = 14.4 px, on the 1/64 px grid the measurer keeps metrics on,
        // which `line_height` snaps to: 922 / 64.
        let line = theme
            .palantir
            .text
            .with_font_size(theme.font.caption)
            .line_height();
        assert_eq!(line, 922.0 / 64.0);
        let pad = card::band_padding(&theme);
        assert_eq!(footer.size.h, pad.sums().h + line, "one row");
        let texts: Vec<String> = h.ui.widgets(None).into_iter().map(|w| w.text).collect();
        assert!(!texts.iter().any(|text| text == "Failed"), "{texts:?}");
    }

    /// A node that never ran says so in a footer, so its height is the one a run leaves it at:
    /// the end of a run moves nothing on the canvas.
    #[test]
    fn a_node_keeps_its_height_when_a_run_ends() {
        use scenarium::{NodeExecStatus, NodeStatus, RamUsage, RunSummary};

        let mut h = CanvasHarness::new(DocumentFixture::probes(1));
        let node_id = h.node(0);
        h.prime(2);
        assert!(
            h.ui.rect(wid::footer(node_id)).is_some(),
            "an idle node has a footer"
        );
        let before =
            h.ui.rect(wid::body(node_id))
                .expect("the body recorded")
                .size
                .h;
        h.ctx.run_state.complete(RunSummary {
            nodes: vec![NodeStatus {
                node_id,
                status: Some(NodeExecStatus::Executed { elapsed_secs: 0.25 }),
                ram: RamUsage::default(),
            }],
            ..RunSummary::default()
        });
        h.prime(2);
        let after =
            h.ui.rect(wid::body(node_id))
                .expect("the body recorded")
                .size
                .h;
        assert_eq!(before, after);
    }
}
