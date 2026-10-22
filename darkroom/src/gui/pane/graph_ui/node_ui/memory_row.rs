//! The node body's memory footer: a strip below the ports reporting the RAM
//! (system) and VRAM (GPU) the node's cached output holds this run. Rendered
//! only when the node retains bytes — a zero pool is dropped and an idle node
//! shows no strip at all, matching the window status bar.

use palantir::prelude::*;
use scenarium::RamUsage;

use crate::gui::graph_ctx::node_ctx::NodeCtx;
use crate::gui::theme::Theme;
use crate::gui::widgets::format::bytes::Bytes;
use crate::gui::widgets::support;
use crate::gui::widgets::support::{CARD_FOOTER_PAD_X, CARD_FOOTER_PAD_Y, LabeledValue};

const DOT: f32 = 6.0;
const BAR_H: f32 = 3.0;

/// Draw the node's memory footer, or nothing when it holds no RAM. `node.ram()`
/// is mirrored from the run cache; each pool shows only when non-zero.
pub(super) fn show(ui: &mut Ui, ncx: NodeCtx<'_>) {
    let node = ncx;
    let ram = node.ram();
    if ram.total() == 0 {
        return;
    }
    let theme = ncx.theme();
    // Same inner radius the header rounds to (`CardTheme::inner_radius`),
    // not the card's raw outer `corner_radius` — else this strip's
    // corner leaves a wedge of body fill showing past the border stroke.
    let r = theme.card.inner_radius();

    Panel::vstack()
        .id_salt("node_mem")
        .size((Sizing::FILL, Sizing::HUG))
        .padding(Spacing::xy(CARD_FOOTER_PAD_X, CARD_FOOTER_PAD_Y))
        .gap(5.0)
        // Round only the bottom corners so the strip seats into the node's
        // rounded bottom — the header rounds the top the same way.
        .bg(support::footer_bg(theme, r))
        .show(ui, |ui| {
            Panel::hstack()
                .id_salt("node_mem_meters")
                .size((Sizing::FILL, Sizing::HUG))
                .gap(12.0)
                .child_align(Align::v(VAlign::Center))
                .show(ui, |ui| {
                    for pool in Pool::held(theme, ram) {
                        pool.meter(ui, theme);
                    }
                });
            proportion_bar(ui, theme, ram);
        });
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
            .gap(5.0)
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
            .bg(Bg::rounded(self.hue, Corners::all(BAR_H * 0.5)))
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
        .bg(Bg::rounded(hue, Corners::all(DOT * 0.5)))
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
