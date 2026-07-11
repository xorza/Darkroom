//! Shared record-time scraps used across the GUI tree: text styles
//! derived from the theme base, small filled/stroked glyph primitives,
//! and layout spacers. Chrome-level composition (pills, chips) lives in
//! [`toolbar`](crate::gui::widgets::toolbar).

use aperture::{Color, Configure, Corners, Panel, Rect, Shape, Sizing, Stroke, TextStyle, Ui};

use crate::gui::theme::Theme;

/// The theme's base text restyled to `px`.
pub(crate) fn sized_text(ui: &Ui, px: f32) -> TextStyle {
    TextStyle {
        font_size_px: px,
        ..ui.theme.text
    }
}

/// [`sized_text`] in an explicit ink.
pub(crate) fn colored_text(ui: &Ui, color: Color, px: f32) -> TextStyle {
    TextStyle {
        color,
        ..sized_text(ui, px)
    }
}

/// [`colored_text`] in the shared de-emphasized ink — the common
/// readout/label style.
pub(crate) fn muted_text(ui: &Ui, theme: &Theme, px: f32) -> TextStyle {
    colored_text(ui, theme.colors.text_muted, px)
}

/// A filled rounded rect — the fill sibling of [`stroked_rect`].
pub(crate) fn filled_rect(ui: &mut Ui, rect: Rect, radius: f32, color: Color) {
    ui.add_shape(Shape::rect(rect).corners(Corners::all(radius)).fill(color));
}

/// A rounded-rect outline (transparent fill, `color` stroke of `width`).
pub(crate) fn stroked_rect(ui: &mut Ui, rect: Rect, radius: f32, color: Color, width: f32) {
    ui.add_shape(
        Shape::rect(rect)
            .corners(Corners::all(radius))
            .stroke(Stroke::solid(color, width)),
    );
}

/// A small filled circle of radius `r` centered at `(cx, cy)`.
pub(crate) fn dot(ui: &mut Ui, cx: f32, cy: f32, r: f32, color: Color) {
    filled_rect(ui, Rect::new(cx - r, cy - r, 2.0 * r, 2.0 * r), r, color);
}

/// The shared rounded-rect outline glyphs frame their contents in,
/// centered in an `s`-sized button box.
pub(crate) fn frame(ui: &mut Ui, s: f32, color: Color) {
    let w = s * 0.62;
    let o = (s - w) * 0.5;
    stroked_rect(ui, Rect::new(o, o, w, w), s * 0.08, color, s * 0.06);
}

/// An empty stretch panel pushing the following hstack siblings to the
/// far edge. `salt` keeps sibling spacers' ids distinct.
pub(crate) fn hspacer(ui: &mut Ui, salt: &'static str) {
    Panel::hstack()
        .id_salt(salt)
        .size((Sizing::FILL, Sizing::Hug))
        .show(ui, |_| {});
}
