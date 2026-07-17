//! Shared record-time scraps used across the GUI tree: text styles
//! derived from the theme base, small filled/stroked glyph primitives,
//! and layout spacers. Chrome-level composition (pills, chips) lives in
//! [`toolbar`](crate::gui::widgets::toolbar).

use std::borrow::Cow;

use aperture::{
    Background, Color, Configure, Corners, FontFamily, Panel, Rect, ResponseSnapshot, Shape,
    Sizing, Stroke, Text, TextStyle, Tooltip, Ui,
};

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

/// [`sized_text`] in the monospace family — tabular readouts (byte figures,
/// dimensions) that should hold a column rather than proportionally kern.
pub(crate) fn mono_text(ui: &Ui, px: f32) -> TextStyle {
    TextStyle {
        family: FontFamily::Mono,
        ..sized_text(ui, px)
    }
}

/// A muted micro-label immediately followed by its mono-styled value, as two
/// direct `Text` widgets (no wrapping panel — draw it inside the caller's
/// own panel so its `gap` spaces the pair like any other sibling). Shared
/// shape behind a node body's memory footer
/// ([`crate::gui::node::memory_row::meter`]) and the pin-preview widget's
/// image-info facts.
pub(crate) fn labeled_value(ui: &mut Ui, theme: &Theme, label: &str, value: String) {
    Text::new(label.to_owned())
        .style(muted_text(ui, theme, 8.5))
        .show(ui);
    Text::new(value).style(mono_text(ui, 10.5)).show(ui);
}

/// A read-only "fact strip" footer's background: the chrome fill, rounded
/// on only its bottom two corners so it seats under whatever rounds the
/// top (a header bar, or the card's own top edge). Shared by a node body's
/// memory footer and the pin-preview widget's image-info footer.
pub(crate) fn footer_background(theme: &Theme, corner_radius: f32) -> Background {
    Background::rounded(
        theme.colors.chrome_fill,
        Corners::new(0.0, 0.0, corner_radius, corner_radius),
    )
}

/// A card's title-bar background: the header fill, rounded on only its top
/// two corners so it seats inside the card's own outer stroke — `corner_radius`
/// should be [`Theme::card_inner_radius`](crate::gui::theme::Theme::card_inner_radius),
/// not the card's outer radius, else the band's corner leaves a wedge of the
/// card's plain fill showing through. Shared by a node body's header and the
/// pin-preview widget's title bar.
pub(crate) fn header_background(theme: &Theme, corner_radius: f32) -> Background {
    Background::rounded(
        theme.colors.header_fill,
        Corners::new(corner_radius, corner_radius, 0.0, 0.0),
    )
}

/// Horizontal/vertical padding of a card's header band — shared by a node
/// body's header and the pin-preview widget's title bar so the two read as
/// the same strip at a different width, not two independently-tuned bars.
pub(crate) const CARD_HEADER_PAD_X: f32 = 8.0;
pub(crate) const CARD_HEADER_PAD_Y: f32 = 7.0;

/// Horizontal/vertical padding of a card's fact-strip footer — shared by a
/// node body's memory footer and the pin-preview widget's image-info footer.
pub(crate) const CARD_FOOTER_PAD_X: f32 = 10.0;
pub(crate) const CARD_FOOTER_PAD_Y: f32 = 6.0;

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
        .size((Sizing::FILL, Sizing::HUG))
        .show(ui, |_| {});
}

/// Shows `tip` as a hover tooltip anchored to `snapshot`, unless `tip` is
/// empty. Takes an already-snapshotted response (rather than `&Response<'_>`)
/// so a caller that just finished building the widget can end its own `ui`
/// borrow (via `.snapshot()`) before this one needs `ui` back mutably to
/// record the tooltip. Shared by every chip/badge/port-glyph that pairs a
/// widget with a hover tooltip. `Cow` so the common `&'static str` tips
/// don't allocate per frame.
pub(crate) fn tooltip_after(
    ui: &mut Ui,
    snapshot: &ResponseSnapshot,
    tip: impl Into<Cow<'static, str>>,
) {
    let tip = tip.into();
    if !tip.is_empty() {
        Tooltip::on(snapshot).text(tip).show(ui);
    }
}
