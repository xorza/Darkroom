//! [`SpaceScale`]: the theme's spacing steps.

/// The spacing steps every padding, gap and margin reads — the spatial half of a
/// [`Theme`](crate::gui::theme::Theme), beside the [`TypeScale`](crate::gui::theme::type_scale::TypeScale).
///
/// A 4 px grid, doubling past 8: steps the eye tells apart, so two surfaces side by side line up
/// on one of them. A literal at a call site is a missing step, not a local decision. The bridge
/// hands the same steps to palantir's widgets.
#[derive(Clone, Debug)]
pub(crate) struct SpaceScale {
    /// Inside a dense cluster: icon to icon, a pill's padding, a tight row.
    pub(crate) xs: f32,
    /// The default gap between siblings, and a control's horizontal padding.
    pub(crate) sm: f32,
    /// A card's horizontal padding, and the gap between groups.
    pub(crate) md: f32,
    /// A floating surface's distance from the window edge.
    pub(crate) lg: f32,
    /// Between sections of a page.
    pub(crate) xl: f32,
    /// A page's margin.
    pub(crate) xxl: f32,
}

impl SpaceScale {
    pub(super) const DEFAULT: Self = Self {
        xs: 4.0,
        sm: 8.0,
        md: 12.0,
        lg: 16.0,
        xl: 24.0,
        xxl: 32.0,
    };
}
