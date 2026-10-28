//! A node card's pieces: the chrome of its fact-strip footer, the padding it shares with the
//! header, and the values a footer shows.

use std::fmt::Display;

use palantir::{Chrome, Corners, Spacing, Text, Ui, fmt};

use crate::gui::theme::Theme;
use crate::gui::widgets::support;

/// The padding of a card's header and of its fact-strip footer: one value, so a header and a
/// footer on the same card can't be tuned apart by accident.
pub(crate) fn band_padding(theme: &Theme) -> Spacing {
    Spacing::xy(theme.space.md, theme.space.sm)
}

/// One mono-styled value, unlabeled — for a figure whose own shape
/// (`1920×1080`, `RGBA · 8-bit`) already says what it is.
///
/// `value` renders on demand — a `Display` figure, or a `format_args!` the
/// caller assembles at the call site — and lands in the record pass's text
/// arena, so a per-frame fact strip builds no `String`.
pub(crate) fn bare_value(ui: &mut Ui, theme: &Theme, value: impl Display) {
    let style = support::figure_text(ui, theme);
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
    /// footer and a preview node's image-info footer.
    pub(crate) fn show(self, ui: &mut Ui, theme: &Theme) {
        Text::new(self.label)
            .style(&support::muted_text(ui, theme, theme.font.caption))
            .show(ui);
        bare_value(ui, theme, self.value);
    }
}

/// A read-only "fact strip" footer's chrome: the surround fill, rounded on only its bottom two
/// corners, at the card's inner radius, so it seats inside the card's stroke. Shared by a node
/// body's footer and a preview node's image-info footer.
pub(crate) fn footer_chrome(theme: &Theme) -> Chrome {
    let r = theme.card_inner_radius();
    Chrome::rounded(theme.colors.surround_bg, Corners::new(0.0, 0.0, r, r))
}
