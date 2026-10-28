//! [`CardTheme`]: the elevated rounded surface node bodies and the inspector's cards read.

use palantir::RgbaF32;

use crate::gui::theme::palette::Palette;

/// An elevated rounded surface: a node's body and an inspector card. Its radius is the theme's
/// `radius.card` and its shadow `elevation.resting`;
/// [`Theme::card_chrome`](crate::gui::theme::Theme::card_chrome) puts them together.
#[derive(Clone, Debug)]
pub(crate) struct CardTheme {
    /// Body fill.
    pub(crate) fill: RgbaF32,
    /// Resting outline. Transparent: the resting shadow carries the edge, and the stroke slot is
    /// reserved for the selection, breaker and missing colors.
    pub(crate) border: RgbaF32,
    /// Two steps over `fill`: the lift behind a hovered button on a card or a toolbar.
    pub(crate) lift: RgbaF32,
    /// The outline's width, at rest and selected alike, so selecting a card never resizes it:
    /// only its color changes.
    pub(crate) border_width: f32,
    /// A node body's smallest width, so a node with short port labels keeps a legible header.
    pub(crate) min_width: f32,
    /// A preview node's smallest width, room for a thumbnail.
    pub(crate) preview_min_width: f32,
}

impl CardTheme {
    pub(super) const fn from_palette(p: &Palette) -> Self {
        Self {
            fill: p.node_bg,
            border: p.node_border,
            lift: p.element_strong,
            border_width: 2.0,
            min_width: 160.0,
            preview_min_width: 240.0,
        }
    }
}
