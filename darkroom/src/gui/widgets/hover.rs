//! [`Hover`]: whether the pointer is over a widget.

use palantir::RgbaF32;

use crate::gui::theme::color;

/// Whether the pointer is over a widget — a node body, a port's glyph, or one
/// a wire drag snaps to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Hover {
    Rest,
    Over,
}

impl From<bool> for Hover {
    fn from(hovered: bool) -> Self {
        if hovered { Self::Over } else { Self::Rest }
    }
}

impl Hover {
    /// `c` as it paints in this state: lifted through
    /// [`color::hover_lift`] while hovered.
    pub(crate) const fn lift(self, c: RgbaF32) -> RgbaF32 {
        match self {
            Self::Over => color::hover_lift(c),
            Self::Rest => c,
        }
    }
}
