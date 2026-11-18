//! [`Hover`]: whether the pointer is over a widget.

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
