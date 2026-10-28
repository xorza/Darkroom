//! [`SpaceScale`]: the theme's spacing steps.

/// The spacing steps every padding, gap and margin reads — the spatial half of a
/// [`Theme`](crate::gui::theme::Theme), beside the [`TypeScale`](crate::gui::theme::type_scale::TypeScale).
///
/// A 4 px grid, in steps of 4 to 16 and of 8 past it, with a 2 px step under it for the insets
/// of the densest rows: steps the eye tells apart, so two surfaces side by side line up on one of
/// them. A literal at a call site is a missing step, not a local decision. The bridge hands the
/// same steps to palantir's widgets.
#[derive(Clone, Debug)]
pub(crate) struct SpaceScale {
    /// A subtitle under its title, a chip's vertical inset.
    pub(crate) xxs: f32,
    /// Inside a dense cluster: icon to icon, a pill's padding, a tight row.
    pub(crate) xs: f32,
    /// The default gap between siblings, and a field's horizontal padding.
    pub(crate) sm: f32,
    /// A card's and a button's horizontal padding, and the gap between groups.
    pub(crate) md: f32,
    /// A popup's distance from the window edge, and a pane's padding.
    pub(crate) lg: f32,
    /// Between sections of a page, a menu's hint gutter and a modal's padding.
    pub(crate) xl: f32,
}

impl SpaceScale {
    pub(super) const DEFAULT: Self = Self {
        xxs: 2.0,
        xs: 4.0,
        sm: 8.0,
        md: 12.0,
        lg: 16.0,
        xl: 24.0,
    };
}
