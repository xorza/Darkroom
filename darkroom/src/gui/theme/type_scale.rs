//! [`TypeScale`]: the theme's type sizes.

/// Font sizes by tier in the visual hierarchy — the typographic half of a
/// [`Theme`](crate::gui::theme::Theme), beside the [`ChromeColors`](crate::gui::theme::chrome_colors::ChromeColors) palette half and the layout
/// dimensions.
///
/// Named by *prominence*, never by the surface that happens to use a tier, so
/// a new surface picks one by asking how loud its text should be rather than
/// copying whichever number a neighbouring panel reached for. Every size the
/// app draws is here: a literal at a call site is a missing tier, not a local
/// decision.
///
/// Palette-independent, so unlike
/// [`ChromeColors`](crate::gui::theme::chrome_colors::ChromeColors) there is
/// no `from_palette` here: a single [`Self::DEFAULT`] is the whole story.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct TypeScale {
    /// The loudest tier: a floating panel's own heading — the inspector's node
    /// title.
    pub(crate) title: f32,
    /// Default UI text — dock tab labels, menu rows, settings rows, the drag
    /// ghost, the status bar, a settings row's help/error/link line, an
    /// inspector row's value. The tier to reach for when none of the others
    /// has a reason to win.
    pub(crate) body: f32,
    /// Labels on dense surfaces, where body would crowd: inspector port rows,
    /// a node's preview row, the viewer's swatch caption, and the tabular
    /// figures beside them (byte counts, dimensions) — the mono family comes
    /// from [`crate::gui::widgets::support::mono_text`], not from a tier of
    /// its own.
    pub(crate) label: f32,
    /// The caption above a readout — the smallest type that ships.
    pub(crate) caption: f32,
}

impl TypeScale {
    /// The authored scale. Four tiers, each a step the eye can actually
    /// resolve: the 15/14 and 13/12 and 11/10.5 pairs this replaced sat a
    /// half-step or a point apart and read as one size, so the smaller of
    /// each was doing no work its neighbour wasn't.
    ///
    /// Badge glyphs stay out: a `■` sized to an 18px chip box is geometry
    /// that happens to be drawn with a font, tracking the box rather than the
    /// hierarchy, so it stays named beside the box (`BADGE_FONT`).
    pub(super) const DEFAULT: Self = Self {
        title: 15.0,
        body: 13.0,
        label: 11.0,
        caption: 8.5,
    };
}
