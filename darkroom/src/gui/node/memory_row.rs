//! The node body's memory footer: a strip below the ports reporting the RAM
//! (system) and VRAM (GPU) the node's cached output holds this run. Rendered
//! only when the node retains bytes — a zero pool is dropped and an idle node
//! shows no strip at all, matching the window status bar.

use aperture::{Align, Background, Color, Configure, Corners, Panel, Sizing, Spacing, Ui, VAlign};

use crate::gui::widgets::support::{
    CARD_FOOTER_PAD_X, CARD_FOOTER_PAD_Y, footer_background, labeled_value,
};
use scenarium::RamUsage;

use crate::gui::format::fmt_bytes;
use crate::gui::node::RecordCtx;
use crate::gui::scene::SceneNode;
use crate::gui::theme::Theme;

const DOT: f32 = 6.0;
const BAR_H: f32 = 3.0;

/// Draw the node's memory footer, or nothing when it holds no RAM. `node.ram`
/// is mirrored from the run cache; each pool shows only when non-zero.
pub(crate) fn memory_row(ui: &mut Ui, rcx: RecordCtx<'_>, node: &SceneNode) {
    let ram = node.ram;
    if ram.total() == 0 {
        return;
    }
    let theme = rcx.theme;
    // Same inner radius the header rounds to (`Theme::card_inner_radius`),
    // not the card's raw outer `node_corner_radius` — else this strip's
    // corner leaves a wedge of body fill showing past the border stroke.
    let r = theme.card_inner_radius();

    Panel::vstack()
        .id_salt("node_mem")
        .size((Sizing::FILL, Sizing::HUG))
        .padding(Spacing::xy(CARD_FOOTER_PAD_X, CARD_FOOTER_PAD_Y))
        .gap(5.0)
        // Round only the bottom corners so the strip seats into the node's
        // rounded bottom — the header rounds the top the same way.
        .background(footer_background(theme, r))
        .show(ui, |ui| {
            Panel::hstack()
                .id_salt("node_mem_meters")
                .size((Sizing::FILL, Sizing::HUG))
                .gap(12.0)
                .child_align(Align::v(VAlign::Center))
                .show(ui, |ui| {
                    if ram.cpu > 0 {
                        meter(ui, theme, theme.colors.badge_cache, "RAM", ram.cpu);
                    }
                    if ram.gpu > 0 {
                        meter(ui, theme, theme.colors.badge_subgraph, "VRAM", ram.gpu);
                    }
                });
            proportion_bar(ui, theme, ram);
        });
}

/// One pool: a colored dot, an uppercase micro-label, and the byte figure. The
/// value takes the node's default text color; the label is muted.
fn meter(ui: &mut Ui, theme: &Theme, hue: Color, label: &'static str, bytes: usize) {
    Panel::hstack()
        .id_salt(("node_mem_meter", label))
        .size((Sizing::HUG, Sizing::HUG))
        .gap(5.0)
        .child_align(Align::v(VAlign::Center))
        .show(ui, |ui| {
            dot(ui, hue);
            labeled_value(ui, theme, label, fmt_bytes(bytes));
        });
}

/// A small filled circle occupying `DOT`×`DOT` of layout so the label flows
/// after it.
fn dot(ui: &mut Ui, hue: Color) {
    Panel::zstack()
        .id_salt("node_mem_dot")
        .size((Sizing::fixed(DOT), Sizing::fixed(DOT)))
        .background(Background::rounded(hue, Corners::all(DOT * 0.5)))
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
            if ram.cpu > 0 {
                bar_segment(ui, "ram", theme.colors.badge_cache, ram.cpu as f32);
            }
            if ram.gpu > 0 {
                bar_segment(ui, "vram", theme.colors.badge_subgraph, ram.gpu as f32);
            }
        });
}

/// One weighted segment of the proportion bar.
fn bar_segment(ui: &mut Ui, salt: &'static str, hue: Color, weight: f32) {
    Panel::zstack()
        .id_salt(("node_mem_seg", salt))
        .size((Sizing::fill(weight), Sizing::FILL))
        .background(Background::rounded(hue, Corners::all(BAR_H * 0.5)))
        .show(ui, |_ui| {});
}
