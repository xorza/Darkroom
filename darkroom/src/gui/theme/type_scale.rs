//! [`TypeScale`]: the theme's type sizes.

/// Font sizes by tier in the visual hierarchy — the typographic half of a
/// [`Theme`](crate::gui::theme::Theme), beside the
/// [`AppColors`](crate::gui::theme::app_colors::AppColors) palette
/// half and the layout dimensions.
///
/// Named by *prominence*, never by the surface that happens to use a tier, so
/// a new surface picks one by asking how loud its text should be rather than
/// copying whichever number a neighbouring panel reached for. Every size the
/// app draws is here: a literal at a call site is a missing tier, not a local
/// decision.
///
/// Palette-independent, so unlike
/// [`AppColors`](crate::gui::theme::app_colors::AppColors) there is
/// no `from_palette` here: a single [`Self::DEFAULT`] is the whole story.
#[derive(Clone, Debug)]
pub(crate) struct TypeScale {
    /// The loudest tier: a pane's own heading — the inspector's node title.
    pub(crate) title: f32,
    /// Default UI text — dock tab labels, menu rows, settings rows, the drag
    /// ghost, the status bar, a settings row's help/error/link line, an
    /// inspector row's value. The tier to reach for when none of the others
    /// has a reason to win.
    pub(crate) body: f32,
    /// Labels on dense surfaces, where body would crowd: inspector port rows,
    /// a node's preview row, and the tabular
    /// figures beside them (byte counts, dimensions) — the mono family comes
    /// from [`crate::gui::widgets::support::mono_text`], not from a tier of
    /// its own.
    pub(crate) label: f32,
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
        label: 12.0,
    };
}
