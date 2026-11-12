//! [`SmallN`]: the combine a pixel too few frames reached falls back to.

use crate::combine::rejection::Rejection;
use crate::combine::stack_config::combine_method::CombineMethod;

/// Small-sample fallback policy for [`Combine`](crate::Combine). Where fewer than `min_frames`
/// frames reach a pixel the configured [`Combine::method`](crate::Combine::method)'s rejection
/// statistics are unreliable, so the combine uses `fallback` there instead: at every pixel of a
/// stack of fewer frames, and at the pixels of a larger one that only some frames cover, such as a
/// mosaic's edges. `fallback` must be rejection-free (`Median` or `Mean(None)`) so it never needs
/// a fallback of its own.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SmallN {
    /// Frames below which `fallback` replaces the configured method.
    pub min_frames: usize,
    /// The combine method used below `min_frames`.
    pub fallback: CombineMethod,
}

impl SmallN {
    /// No fallback — the method is reliable at any frame count (Winsorized, Trim, Median, plain
    /// mean).
    pub const fn none() -> Self {
        Self {
            min_frames: 0,
            fallback: CombineMethod::Median,
        }
    }

    /// Fall back to the median below `min_frames` frames.
    pub const fn median_below(min_frames: usize) -> Self {
        Self {
            min_frames,
            fallback: CombineMethod::Median,
        }
    }

    /// Whether a pixel of `count` samples combines by `fallback` rather than `method`: there are
    /// too few for its rejection. A method that rejects nothing has nothing to fall back *from*,
    /// and a fallback equal to the method is none, so a method override that inherits a preset's
    /// `min_frames` never swaps a plain mean and a median at small N.
    pub(crate) fn falls_back(&self, method: CombineMethod, count: usize) -> bool {
        let does_rejection =
            matches!(method, CombineMethod::Mean(rejection) if rejection != Rejection::None);
        does_rejection && count < self.min_frames && method != self.fallback
    }
}
