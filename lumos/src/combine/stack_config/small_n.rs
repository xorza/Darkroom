//! [`SmallN`]: the combine a pixel too few frames reached falls back to.

/// Where fewer than `min_frames` frames reach a pixel the rejection's statistics are unreliable,
/// so a [`Combine::RejectedMean`](crate::Combine::RejectedMean) combines the pixel by `fallback`
/// instead: at every pixel of a stack of fewer frames, and at the pixels of a larger one that only
/// some frames cover, such as a mosaic's edges. `min_frames` is at least 2 and at most
/// [`Self::MAX_MIN_FRAMES`], which the configuration's validation holds it to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SmallN {
    /// Frames below which `fallback` replaces the rejected mean.
    pub min_frames: usize,
    pub fallback: Fallback,
}

/// How a pixel too few frames reached is combined: by nothing that rejects, so it needs no
/// fallback of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fallback {
    /// The median, which still rejects one outlier among three.
    Median,
    /// The plain weighted mean.
    Mean,
}

impl SmallN {
    /// The largest floor a fallback takes: past it a rejection's statistics are sound, and the
    /// median's small-sample variance factor is tabled up to it.
    pub const MAX_MIN_FRAMES: usize = 33;

    /// Fall back to the median below `min_frames` frames.
    pub const fn median_below(min_frames: usize) -> Self {
        Self {
            min_frames,
            fallback: Fallback::Median,
        }
    }

    /// Whether a pixel of `count` samples combines by the fallback: too few for the rejection.
    pub(crate) const fn falls_back(self, count: usize) -> bool {
        count < self.min_frames
    }
}
