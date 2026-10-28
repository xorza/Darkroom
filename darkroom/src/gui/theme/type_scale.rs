//! [`TypeScale`]: the theme's type sizes.

/// Font sizes by tier in the visual hierarchy — the typographic scale of a
/// [`Theme`](crate::gui::theme::Theme), beside its space, radius and stroke scales.
///
/// Named by *prominence*, never by the surface that happens to use a tier, so
/// a new surface picks one by asking how loud its text should be rather than
/// copying whichever number a neighbouring panel reached for. Every size the
/// app draws is here: a literal at a call site is a missing tier, not a local
/// decision.
///
/// Palette-independent, as every scale is: a single [`Self::DEFAULT`] is the whole story.
#[derive(Clone, Debug)]
pub(crate) struct TypeScale {
    /// The loudest tier: a pane's own heading — the inspector's node title.
    pub(crate) title: f32,
    /// Default UI text — dock tab labels, menu rows, settings rows, the drag
    /// ghost, the status bar, an inspector row's value. The tier to reach for when none of the
    /// others has a reason to win.
    pub(crate) body: f32,
    /// Secondary text and dense surfaces, where body would crowd: a caption over a group, a
    /// settings field's help, error and link lines, a node's footer and preview row, and the
    /// tabular figures beside them (byte counts, dimensions) — the mono family comes from
    /// [`crate::gui::widgets::support::figure_text`], not from a tier of its own.
    pub(crate) caption: f32,
}

impl TypeScale {
    /// The authored scale, the Windows Fluent 2 ramp (Body 14, Caption 12,
    /// Subtitle 16): two points between tiers, a step the eye resolves, where a
    /// point apart reads as one size. 15/13/11, the macOS and KDE body size, read
    /// small on a large screen at scale 1.5; a whole-UI factor on top is the
    /// interface size preference, not another tier.
    pub(super) const DEFAULT: Self = Self {
        title: 16.0,
        body: 14.0,
        caption: 12.0,
    };
}

/// The advance of every glyph of the figure face, JetBrains Mono, per unit of its size: each
/// glyph's advance in its `hmtx` table is 600 of the 1000 units per em in its `head` table.
const FIGURE_ADVANCE: f32 = 0.6;

/// How far the shaper's sum of f32 advances can land past their exact product: ten figures at
/// 14 px sum past 84 and measure 85. A 64th of a pixel, the text grid palantir snaps to, is far
/// above that error and far below a pixel.
const ADVANCE_SUM_SLACK: f32 = 1.0 / 64.0;

/// The width `glyphs` figures take at `px`, up to the whole pixel palantir measures a run to: the
/// floor of a readout that changes while it is watched, so the line beside it holds still as its
/// digit count changes.
pub(crate) fn figure_width(px: f32, glyphs: u8) -> f32 {
    (f32::from(glyphs) * FIGURE_ADVANCE * px + ADVANCE_SUM_SLACK).ceil()
}
