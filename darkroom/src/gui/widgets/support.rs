//! Shared record-time scraps used across the GUI tree: text styles
//! derived from the theme base, small filled/stroked glyph primitives,
//! and layout spacers. Chrome-level composition (pills, chips) lives in
//! [`toolbar`](crate::gui::widgets::toolbar).

use std::fmt::Display;

use glam::Vec2;

use palantir::{
    Background, RgbaF32, Configure, Corners, FontFamily, Panel, Rect, ResponseSnapshot, Shape,
    Sizing, Stroke, Text, TextInput, TextStyle, Tooltip, Ui, fmt,
};

use crate::gui::theme::Theme;

/// The theme's base text restyled to `px`.
pub(crate) fn sized_text(ui: &Ui, px: f32) -> TextStyle {
    TextStyle {
        font_size_px: px,
        ..ui.theme().text
    }
}

/// [`sized_text`] in an explicit ink.
pub(crate) fn colored_text(ui: &Ui, color: RgbaF32, px: f32) -> TextStyle {
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
        family: FontFamily::MONO,
        ..sized_text(ui, px)
    }
}

/// One mono-styled value, unlabeled — for a figure whose own shape
/// (`1920×1080`, `RGBA · 8-bit`) already says what it is.
///
/// `value` renders on demand — a `Display` figure, or a `format_args!` the
/// caller assembles at the call site — and lands in the record pass's text
/// arena, so a per-frame fact strip builds no `String`.
pub(crate) fn bare_value(ui: &mut Ui, theme: &Theme, value: impl Display) {
    let style = mono_text(ui, theme.text.label);
    Text::new(fmt!(ui, "{value}")).style(&style).show(ui);
}

/// [`bare_value`] behind a muted micro-label, as two direct `Text` widgets
/// (no wrapping panel — draw it inside the caller's own panel so its `gap`
/// spaces the pair like any other sibling). Shared shape behind a node body's
/// memory footer (`gui::pane::graph::node::memory_row`) and a preview node's
/// image-info footer (`gui::pane::graph::node::preview_row`).
pub(crate) fn labeled_value(ui: &mut Ui, theme: &Theme, label: &str, value: impl Display) {
    Text::new(label)
        .style(&muted_text(ui, theme, theme.text.caption))
        .show(ui);
    bare_value(ui, theme, value);
}

/// A read-only "fact strip" footer's background: the chrome fill, rounded
/// on only its bottom two corners so it seats under whatever rounds the
/// top (a header bar, or the card's own top edge). Shared by a node body's
/// memory footer and a preview node's image-info footer.
pub(crate) fn footer_background(theme: &Theme, corner_radius: f32) -> Background {
    Background::rounded(
        theme.colors.chrome_fill,
        Corners::new(0.0, 0.0, corner_radius, corner_radius),
    )
}

/// A card's title-bar background: the header fill, rounded on only its top
/// two corners so it seats inside the card's own outer stroke — `corner_radius`
/// should be [`CardTheme::inner_radius`](crate::gui::theme::card_theme::CardTheme::inner_radius),
/// not the card's outer radius, else the band's corner leaves a wedge of the
/// card's plain fill showing through. Used by a node body's header — the one
/// title bar in the tree, kept here beside its footer peer.
pub(crate) fn header_background(theme: &Theme, corner_radius: f32) -> Background {
    Background::rounded(
        theme.card.header_fill,
        Corners::new(corner_radius, corner_radius, 0.0, 0.0),
    )
}

/// Horizontal/vertical padding of a card's header band, named beside the
/// footer pair below so a header and a footer on the same card can't be
/// tuned apart by accident.
pub(crate) const CARD_HEADER_PAD_X: f32 = 8.0;
pub(crate) const CARD_HEADER_PAD_Y: f32 = 7.0;

/// Horizontal/vertical padding of a card's fact-strip footer — shared by a
/// node body's memory footer and a preview node's image-info footer.
pub(crate) const CARD_FOOTER_PAD_X: f32 = 10.0;
pub(crate) const CARD_FOOTER_PAD_Y: f32 = 6.0;

/// A filled rounded rect — the fill sibling of [`stroked_rect`].
pub(crate) fn filled_rect(ui: &mut Ui, rect: Rect, radius: f32, color: RgbaF32) {
    ui.add_shape(Shape::rect(rect).corners(radius).fill(color));
}

/// A rounded-rect outline (transparent fill, `color` stroke of `width`).
pub(crate) fn stroked_rect(ui: &mut Ui, rect: Rect, radius: f32, color: RgbaF32, width: f32) {
    ui.add_shape(
        Shape::rect(rect)
            .corners(radius)
            .stroke(Stroke::solid(color, width)),
    );
}

/// A small filled circle of radius `r` centered at `(cx, cy)`.
pub(crate) fn dot(ui: &mut Ui, cx: f32, cy: f32, r: f32, color: RgbaF32) {
    filled_rect(ui, Rect::new(cx - r, cy - r, 2.0 * r, 2.0 * r), r, color);
}

/// A right-pointing play triangle inscribed in an `s`-sized box, spanning
/// `fill` of it.
///
/// Nudged right by a fifth of its half-width: a play mark's visual centre sits
/// left of its bounding box's, so a geometrically centred one reads as offset.
/// Its points are pulled in by the corner radius because the SDF rounds by
/// dilating — the glyph grows back out to the intended extents.
///
/// Shared so the two chips carrying this mark — the node header's run chip and
/// the graph toolbar's run button — cannot drift on that geometry. `fill`
/// stays per-caller: a 30px toolbar button carries proportionally less glyph
/// than an 18px header badge.
pub(crate) fn play_triangle(ui: &mut Ui, s: f32, fill: f32, color: RgbaF32) {
    let half_h = s * fill * 0.5;
    let half_w = half_h * (5.0 / 6.0);
    let r = s * 0.05;
    let nudge = half_w * 0.2;
    let c = Vec2::splat(s * 0.5);
    ui.add_shape(
        Shape::triangle(
            c + Vec2::new(nudge - half_w + r, r - half_h),
            c + Vec2::new(nudge - half_w + r, half_h - r),
            c + Vec2::new(nudge + half_w - r, 0.0),
        )
        .radius(r)
        .fill(color),
    );
}

/// The shared rounded-rect outline glyphs frame their contents in,
/// centered in an `s`-sized button box.
pub(crate) fn frame(ui: &mut Ui, s: f32, color: RgbaF32) {
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

/// Shows `tip` as a hover tooltip anchored to `snapshot`; `None` records
/// none. Takes an already-snapshotted response (rather than `&Response<'_>`)
/// so a caller that just finished building the widget can end its own `ui`
/// borrow (via `.snapshot()`) before this one needs `ui` back mutably to
/// record the tooltip. Shared by every chip/badge/port-glyph that pairs a
/// widget with a hover tooltip.
///
/// `Option` rather than an empty-string sentinel: "this widget has no
/// tooltip" is a different fact from "its tooltip is blank", and the port
/// glyphs — the one family that decides per frame — now say which they mean.
/// Any text form is accepted: a `&'static str` tip stays borrowed until the
/// bubble actually records, and a [`fmt!`] one is already in the pass's arena.
pub(crate) fn tooltip_after<'a>(
    ui: &mut Ui,
    snapshot: &'a ResponseSnapshot,
    tip: Option<impl Into<TextInput<'a>>>,
) {
    if let Some(tip) = tip {
        Tooltip::on(snapshot).label(tip).show(ui);
    }
}
