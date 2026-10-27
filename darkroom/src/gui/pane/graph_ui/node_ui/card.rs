//! A node card's pieces: the chrome of its header band and its fact-strip footer, their
//! padding, and the values a footer shows.

use std::fmt::Display;

use palantir::prelude::*;

use crate::gui::theme::Theme;
use crate::gui::widgets::support;

/// Horizontal/vertical padding of a card's header band, named beside the
/// footer pair below so a header and a footer on the same card can't be
/// tuned apart by accident.
pub(crate) const CARD_HEADER_PAD_X: f32 = 8.0;
pub(crate) const CARD_HEADER_PAD_Y: f32 = 7.0;

/// Horizontal/vertical padding of a card's fact-strip footer — shared by a
/// node body's memory footer and a preview node's image-info footer.
pub(crate) const CARD_FOOTER_PAD_X: f32 = 10.0;
pub(crate) const CARD_FOOTER_PAD_Y: f32 = 6.0;

/// One mono-styled value, unlabeled — for a figure whose own shape
/// (`1920×1080`, `RGBA · 8-bit`) already says what it is.
///
/// `value` renders on demand — a `Display` figure, or a `format_args!` the
/// caller assembles at the call site — and lands in the record pass's text
/// arena, so a per-frame fact strip builds no `String`.
pub(crate) fn bare_value(ui: &mut Ui, theme: &Theme, value: impl Display) {
    let style = support::mono_text(ui, theme.text.label);
    Text::new(fmt!(ui, "{value}")).style(&style).show(ui);
}

/// A value behind a muted micro-label.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LabeledValue<'a, V> {
    pub(crate) label: &'a str,
    pub(crate) value: V,
}

impl<V: Display> LabeledValue<'_, V> {
    /// The value as [`bare_value`] behind its label, as two direct `Text` widgets
    /// (no wrapping panel — draw it inside the caller's own panel so its `gap`
    /// spaces the pair like any other sibling). Shared shape behind a node body's
    /// memory footer and a preview node's image-info footer.
    pub(crate) fn show(self, ui: &mut Ui, theme: &Theme) {
        Text::new(self.label)
            .style(&support::muted_text(ui, theme, theme.text.caption))
            .show(ui);
        bare_value(ui, theme, self.value);
    }
}

/// A read-only "fact strip" footer's chrome: the surround fill, rounded
/// on only its bottom two corners so it seats under whatever rounds the
/// top (a header bar, or the card's own top edge). Shared by a node body's
/// memory footer and a preview node's image-info footer.
pub(crate) fn footer_chrome(theme: &Theme, corner_radius: f32) -> Chrome {
    Chrome::rounded(
        theme.colors.surround_fill,
        Corners::new(0.0, 0.0, corner_radius, corner_radius),
    )
}

/// A card's title-bar chrome: the header fill, rounded on only its top
/// two corners so it seats inside the card's own outer stroke — `corner_radius`
/// should be [`CardTheme::inner_radius`](crate::gui::theme::card_theme::CardTheme::inner_radius),
/// not the card's outer radius, else the band's corner leaves a wedge of the
/// card's plain fill showing through. Used by a node body's header — the one
/// title bar in the tree, kept here beside its footer peer.
pub(crate) fn header_chrome(theme: &Theme, corner_radius: f32) -> Chrome {
    Chrome::rounded(
        theme.card.header_fill,
        Corners::new(corner_radius, corner_radius, 0.0, 0.0),
    )
}
