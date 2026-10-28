//! [`TintScale`]: the strengths of a wash.

/// How strongly an ink washes over the ground under it, composited by
/// [`color::wash`](crate::gui::theme::color::wash): three strengths the eye tells apart, so a
/// tinted surface reads at one of them wherever it sits.
#[derive(Clone, Debug)]
pub(crate) struct TintScale {
    /// A callout's ground: the body ink stays the loudest thing on it.
    pub(crate) faint: f32,
    /// A chip's ground, a focused row, an "on" button: a state the eye should find.
    pub(crate) soft: f32,
    /// An "on" button under the pointer, a step over [`Self::soft`].
    pub(crate) strong: f32,
}

impl TintScale {
    pub(super) const DEFAULT: Self = Self {
        faint: 0.10,
        soft: 0.14,
        strong: 0.26,
    };
}
