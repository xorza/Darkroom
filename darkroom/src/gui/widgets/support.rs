//! Shared record-time scraps used across the GUI tree: text styles
//! derived from the theme base, small filled/stroked glyph primitives,
//! and layout spacers. Chrome-level composition (pills, chips) lives in
//! [`pill`](crate::gui::widgets::pill).

use palantir::prelude::*;
use palantir::widget::{RectShape, Shape};
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

/// A small circle of radius `r` centered on `center`, for the caller to fill.
pub(crate) fn dot(center: Vec2, r: f32) -> RectShape {
    Shape::rect(Rect::new(center.x - r, center.y - r, 2.0 * r, 2.0 * r)).corners(r)
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
/// record the tooltip. Shared by every chip, icon button and port glyph that pairs a
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
