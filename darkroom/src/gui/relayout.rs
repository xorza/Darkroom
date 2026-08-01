//! Whether a frame owes palantir a second layout pass.

use std::ops::{BitOr, BitOrAssign};

/// Whether anything a pass did stranded `CanvasGeometry`'s cross-frame caches,
/// so the frame owes a relayout.
///
/// Named rather than a `bool` because this travels a long way: every phase of
/// [`Session::frame`](crate::gui::app::session::Session::frame) reports one
/// upward, each document mutation produces one, and `App` spends the total on
/// a single `request_relayout`. At a `-> bool` signature there is nothing to
/// say whether `true` means "did something", "succeeded", or "needs a pass" —
/// and neighbouring methods on the same types already return the first two.
///
/// [`Default`] is [`NotNeeded`](Self::NotNeeded), so a pass that reports
/// nothing costs the frame nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Relayout {
    /// Nothing the layout engine measures changed shape; this frame's cascade
    /// still describes the tree.
    #[default]
    NotNeeded,
    /// Something remeasured — a node's ports moved, a canvas appeared, a title
    /// grew — so the cached geometry read next frame would be stale.
    Needed,
}

impl Relayout {
    /// Lift a predicate that already answers the question, for the passes that
    /// derive it from something else — an `UndoStep`'s own verdict, or a
    /// canvas noticing it just came back on screen.
    pub(crate) fn needed_if(stranded: bool) -> Self {
        if stranded {
            Self::Needed
        } else {
            Self::NotNeeded
        }
    }
}

impl BitOr for Relayout {
    type Output = Self;

    /// Accumulating: one pass needing a relayout is enough for the frame,
    /// however many others reported nothing.
    fn bitor(self, rhs: Self) -> Self {
        Self::needed_if(self == Self::Needed || rhs == Self::Needed)
    }
}

impl BitOrAssign for Relayout {
    fn bitor_assign(&mut self, rhs: Self) {
        *self = *self | rhs;
    }
}
