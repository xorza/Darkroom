//! The node body's memory footer: a strip below the ports reporting the RAM
//! (system) and VRAM (GPU) the node's cached output holds this run. Rendered
//! only when the node retains bytes — a zero pool is dropped and an idle node
//! shows no strip at all, matching the window status bar.

use aperture::{
    Align, Background, Color, Configure, Corners, FontFamily, Panel, Sizing, Spacing, Text,
    TextStyle, Ui, VAlign,
};
use scenarium::data::RamUsage;

use crate::gui::format::fmt_bytes;
use crate::gui::node::RecordCtx;
use crate::gui::scene::SceneNode;
use crate::gui::theme::Theme;

const VALUE_FONT: f32 = 10.5;
const LABEL_FONT: f32 = 8.5;
const DOT: f32 = 6.0;
const BAR_H: f32 = 3.0;
const PAD_X: f32 = 10.0;
const PAD_Y: f32 = 6.0;

/// Draw the node's memory footer, or nothing when it holds no RAM. `node.ram`
/// is mirrored from the run cache; each pool shows only when non-zero.
pub(crate) fn memory_row(ui: &mut Ui, rcx: RecordCtx<'_>, node: &SceneNode) {
    let ram = node.ram;
    if ram.total() == 0 {
        return;
    }
    let theme = rcx.theme;
    let r = theme.node_corner_radius;

    Panel::vstack()
        .id_salt("node_mem")
        .size((Sizing::FILL, Sizing::Hug))
        .padding(Spacing::xy(PAD_X, PAD_Y))
        .gap(5.0)
        .background(Background {
            fill: theme.colors.chrome_fill.into(),
            // Round only the bottom corners so the strip seats into the node's
            // rounded bottom — the header rounds the top the same way.
            corners: Corners::new(0.0, 0.0, r, r),
            ..Default::default()
        })
        .show(ui, |ui| {
            Panel::hstack()
                .id_salt("node_mem_meters")
                .size((Sizing::FILL, Sizing::Hug))
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
        .size((Sizing::Hug, Sizing::Hug))
        .gap(5.0)
        .child_align(Align::v(VAlign::Center))
        .show(ui, |ui| {
            dot(ui, hue);
            Text::new(label)
                .style(TextStyle {
                    color: theme.colors.text_muted,
                    font_size_px: LABEL_FONT,
                    ..ui.theme.text
                })
                .show(ui);
            Text::new(fmt_bytes(bytes))
                .style(TextStyle {
                    font_size_px: VALUE_FONT,
                    family: FontFamily::Mono,
                    ..ui.theme.text
                })
                .show(ui);
        });
}

/// A small filled circle occupying `DOT`×`DOT` of layout so the label flows
/// after it.
fn dot(ui: &mut Ui, hue: Color) {
    Panel::zstack()
        .id_salt("node_mem_dot")
        .size((Sizing::Fixed(DOT), Sizing::Fixed(DOT)))
        .background(Background {
            fill: hue.into(),
            corners: Corners::all(DOT * 0.5),
            ..Default::default()
        })
        .show(ui, |_ui| {});
}

/// A thin two-tone bar encoding the CPU:GPU split — each segment's width is its
/// byte share (weighted `Fill`). A single-pool footer reads as one full-width
/// color.
fn proportion_bar(ui: &mut Ui, theme: &Theme, ram: RamUsage) {
    Panel::hstack()
        .id_salt("node_mem_bar")
        .size((Sizing::FILL, Sizing::Fixed(BAR_H)))
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
        .size((Sizing::Fill(weight), Sizing::FILL))
        .background(Background {
            fill: hue.into(),
            corners: Corners::all(BAR_H * 0.5),
            ..Default::default()
        })
        .show(ui, |_ui| {});
}
