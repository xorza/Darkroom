//! [`RadiusScale`]: the theme's corner radii.

/// How round a surface is, by what it is, so a mark, a control, a card and a floating surface
/// each read as their kind wherever they sit.
#[derive(Clone, Debug)]
pub(crate) struct RadiusScale {
    /// A glyph-sized shape: a port's arrow, a swatch.
    pub(crate) mark: f32,
    /// A field, a button, a menu row, a tooltip.
    pub(crate) control: f32,
    /// A node, a tab chip, a toolbar chip, a pane's card.
    pub(crate) card: f32,
    /// A menu, a popup, a toolbar pill. A toolbar pill is concentric with the card-rounded chips
    /// inside it at the [`SpaceScale::xs`](crate::gui::theme::space_scale::SpaceScale::xs)
    /// padding.
    pub(crate) float: f32,
}

impl RadiusScale {
    pub(super) const DEFAULT: Self = Self {
        mark: 2.0,
        control: 4.0,
        card: 6.0,
        float: 10.0,
    };
}
