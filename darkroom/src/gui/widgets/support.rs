//! Shared record-time scraps used across the GUI tree: text styles
//! derived from the theme base, small filled/stroked glyph primitives,
//! and layout spacers. Chrome-level composition (pills, chips) lives in
//! [`pill`](crate::gui::widgets::pill).

use palantir::prelude::*;
use palantir::widget::{RectShape, Shape, TriangleShape};
use palantir::{FontFamily, ResponseSnapshot, TextInput};

use crate::gui::theme::Theme;

/// The theme's base text restyled to `px`.
pub(crate) fn sized_text(ui: &Ui, px: f32) -> TextStyle {
    TextStyle {
        font_size: px,
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

/// The gap between the items of one row on a node card: a port's glyph and
/// its label, a header's chips.
pub(crate) const ROW_GAP: f32 = 4.0;

/// A small circle of radius `r` centered on `center`, for the caller to fill.
pub(crate) fn dot(center: Vec2, r: f32) -> RectShape {
    Shape::rect(Rect::new(center.x - r, center.y - r, 2.0 * r, 2.0 * r)).corners(r)
}

/// A right-pointing play triangle inscribed in an `s`-sized box, spanning
/// `fill` of it, for the caller to fill.
///
/// Nudged right by a fifth of its half-width: a play mark's visual center sits
/// left of its bounding box's, so a geometrically centered one reads as offset.
/// Its points are pulled in by the corner radius because the SDF rounds by
/// dilating — the glyph grows back out to the intended extents.
///
/// Shared so the two chips carrying this mark — the node header's run chip and
/// the graph toolbar's run button — cannot drift on that geometry. `fill`
/// stays per-caller: a 30px toolbar button carries proportionally less glyph
/// than an 18px header badge.
pub(crate) fn play_triangle(s: f32, fill: f32) -> TriangleShape {
    play_triangle_at(s, Vec2::splat(s * 0.5), fill)
}

/// [`play_triangle`] centered on `c` rather than on the box — for a mark that
/// shares its box with another, like the event-loop button's bar.
pub(crate) fn play_triangle_at(s: f32, c: Vec2, fill: f32) -> TriangleShape {
    let half_h = s * fill * 0.5;
    let half_w = half_h * (5.0 / 6.0);
    let r = s * 0.05;
    let nudge = half_w * 0.2;
    Shape::triangle(
        c + Vec2::new(nudge - half_w + r, r - half_h),
        c + Vec2::new(nudge - half_w + r, half_h - r),
        c + Vec2::new(nudge + half_w - r, 0.0),
    )
    .radius(r)
}

/// An empty stretch panel pushing the following hstack siblings to the
/// far edge. `salt` keeps sibling spacers' ids distinct.
pub(crate) fn hspacer(ui: &mut Ui, salt: &'static str) {
    Panel::hstack()
        .id_salt(salt)
        .size((Sizing::FILL, Sizing::HUG))
        .show(ui, |_| {});
}

/// Shows `tooltip` as a hover tooltip anchored to `snapshot`; `None` records
/// none. Takes an already-snapshotted response (rather than `&Response<'_>`)
/// so a caller that just finished building the widget can end its own `ui`
/// borrow (via `.snapshot()`) before this one needs `ui` back mutably to
/// record the tooltip. Shared by every chip/badge/port-glyph that pairs a
/// widget with a hover tooltip.
///
/// `Option` rather than an empty-string sentinel: "this widget has no
/// tooltip" is a different fact from "its tooltip is blank", and the port
/// glyphs — the one family that decides per frame — now say which they mean.
/// Any text form is accepted: a `&'static str` tooltip stays borrowed until the
/// bubble actually records, and a [`fmt!`] one is already in the pass's arena.
pub(crate) fn tooltip_after<'a>(
    ui: &mut Ui,
    snapshot: &'a ResponseSnapshot,
    tooltip: Option<impl Into<TextInput<'a>>>,
) {
    if let Some(tooltip) = tooltip {
        Tooltip::on(snapshot, tooltip).show(ui);
    }
}
