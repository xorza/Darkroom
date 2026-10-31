//! Shared record-time scraps used across the GUI tree: text styles derived from the theme base,
//! a dot and layout spacers. A toolbar's pill lives in [`pill`](crate::gui::widgets::pill), its
//! chips in [`chip`](crate::gui::widgets::chip).

use palantir::{
    Align, Block, Chrome, Configure, Corners, FontFamily, FontWeight, Rect, ResponseSnapshot,
    RgbaF32, Sizing, TextInput, TextStyle, TextWrap, Tooltip, Ui,
};

use glam::Vec2;
use palantir::widget::{RectShape, Shape, TextRun};

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
    colored_text(ui, theme.ink.muted, px)
}

/// The body tier, the size every row label and value reads at.
pub(crate) fn body_text(ui: &Ui, theme: &Theme) -> TextStyle {
    sized_text(ui, theme.font.body)
}

/// The title tier in bold: a node's name in the inspector, a dialog's title. The canvas draws a
/// node's name bold too, at the body tier its row has room for.
pub(crate) fn title_text(ui: &Ui, theme: &Theme) -> TextStyle {
    TextStyle {
        weight: FontWeight::BOLD,
        ..sized_text(ui, theme.font.title)
    }
}

/// A caption over a group: the caption tier, semibold, in the muted ink — an inspector card's, a
/// preferences section's, a palette category's.
pub(crate) fn caption_text(ui: &Ui, theme: &Theme) -> TextStyle {
    TextStyle {
        weight: FontWeight::SEMI_BOLD,
        ..muted_text(ui, theme, theme.font.caption)
    }
}

/// A figure: the caption tier in the monospace family, so digits hold their column.
pub(crate) fn figure_text(ui: &Ui, theme: &Theme) -> TextStyle {
    mono_text(ui, theme.font.caption)
}

/// [`sized_text`] in the monospace family — tabular readouts (byte figures,
/// dimensions) that should hold a column rather than proportionally kern.
pub(crate) fn mono_text(ui: &Ui, px: f32) -> TextStyle {
    TextStyle {
        family: FontFamily::MONO,
        ..sized_text(ui, px)
    }
}

/// The width `glyphs` figures take in `style`, as palantir measures them: the floor of a readout
/// that changes while it is watched, so the line beside it holds still as its digit count
/// changes. Measured on zeros, as every figure of the mono face advances alike.
pub(crate) fn figure_width(ui: &mut Ui, style: &TextStyle, glyphs: u8) -> f32 {
    const FIGURES: &str = "0000000000000000";
    let text = &FIGURES[..usize::from(glyphs)];
    ui.probe_text(TextRun {
        text,
        font: style.font(),
        wrap: TextWrap::SingleLine,
        align: Align::LEFT,
        max_width: None,
    })
    .size()
    .w
}

/// A small circle of radius `r` centered on `center`, for the caller to fill.
pub(crate) fn dot(center: Vec2, r: f32) -> RectShape {
    Shape::rect(Rect::new(center.x - r, center.y - r, 2.0 * r, 2.0 * r)).corners(r)
}

/// A filled circle of the theme's `icon.dot` side that takes its side in layout, so a label flows
/// after it: the unsaved mark, a memory pool's key. [`dot`] is the bare shape, with no layout.
pub(crate) fn dot_cell(ui: &mut Ui, salt: &'static str, theme: &Theme, hue: RgbaF32) {
    let side = theme.icon.dot;
    Block::new()
        .id_salt(salt)
        .size(side)
        .chrome(Chrome::rounded(hue, Corners::all(side * 0.5)))
        .show(ui);
}

/// An empty stretch panel pushing the following hstack siblings to the
/// far edge. `salt` keeps sibling spacers' ids distinct.
pub(crate) fn hspacer(ui: &mut Ui, salt: &'static str) {
    Block::new()
        .id_salt(salt)
        .size((Sizing::FILL, Sizing::HUG))
        .show(ui);
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
