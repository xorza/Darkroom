//! [`IconScale`]: the theme's icon boxes, and the grid their artwork is drawn on.

use palantir::TextSteps;

/// The side of the grid an icon's artwork is drawn on, scaled to the box it paints in.
///
/// 24 units with a 2-unit stroke, the grid and weight of the open icon sets (Lucide, Feather,
/// Material Symbols at weight 400), so a 16 px icon strokes at 1.33 px and a 18 px one at 1.5 px.
pub(crate) const GRID: f32 = 24.0;
pub(crate) const STROKE: f32 = 2.0;

/// The span of the cross icon's arms on the grid, centered.
pub(crate) const CROSS_SPAN: f32 = 12.0;

/// The stroke an icon drawn in a `side` box wears.
pub(crate) const fn stroke_width(side: f32) -> f32 {
    side / GRID * STROKE
}

/// The span of the cross icon's arms in a `side` box, for a widget that draws the cross itself.
pub(crate) const fn cross_span(side: f32) -> f32 {
    side / GRID * CROSS_SPAN
}

/// An icon's side and the room around it in its button, each side.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct IconBox {
    pub(crate) icon: f32,
    pub(crate) inset: f32,
}

impl IconBox {
    /// The button's side: the icon and its inset on both sides.
    pub(crate) const fn side(self) -> f32 {
        self.icon + 2.0 * self.inset
    }
}

/// The icon sizes, by where an icon sits. Every icon of the set strokes on a 24-unit grid, so a
/// tier is a box size, not a weight.
#[derive(Clone, Debug)]
pub(crate) struct IconScale {
    /// A toolbar chip: 18 in 30.
    pub(crate) toolbar: IconBox,
    /// A node header's or a pane's button, a step over the 14 px title beside it: 16 in 28.
    pub(crate) control: IconBox,
    /// An icon in a line of caption text, the size of the caption tier: a footer's status icon, a
    /// tab's close. Its 3 px lift keeps a tab 32 px tall.
    pub(crate) inline: IconBox,
    /// A dot beside a label: the unsaved mark and a memory pool's key.
    pub(crate) dot: f32,
}

impl IconScale {
    /// 6 px a side for a button's icon: a toolbar chip draws 18 in 30, a node header's button 16
    /// in 28, a step over the 14 px title. Refused: 15 in 22, whose 3.5 px read cramped.
    pub(super) const fn new(text: &TextSteps) -> Self {
        Self {
            toolbar: IconBox {
                icon: 18.0,
                inset: 6.0,
            },
            control: IconBox {
                icon: 16.0,
                inset: 6.0,
            },
            inline: IconBox {
                icon: text.caption,
                inset: 3.0,
            },
            dot: 6.0,
        }
    }
}
