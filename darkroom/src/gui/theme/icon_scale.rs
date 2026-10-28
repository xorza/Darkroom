//! [`IconScale`]: the theme's icon boxes.

use crate::gui::theme::type_scale::TypeScale;

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
    /// The boxes the review chose: 6 px a side for a button, as "An icon button gives its icon
    /// 6 px a side" in `darkroom/AGENTS.md` records.
    pub(super) const fn new(text: &TypeScale) -> Self {
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
