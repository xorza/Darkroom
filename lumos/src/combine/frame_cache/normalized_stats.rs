//! [`NormalizedStats`]: the frames' statistics, and the norms that carry them into the combine's
//! units.

use crate::combine::frame_cache::slots::Slots;
use crate::combine::normalization::FrameNorm;

/// Each frame's source statistics in frame order, `stats`, the norms that put the frames on one
/// scale — `None` when every frame is combined as it stands — and the slots both index.
#[derive(Debug)]
pub(crate) struct NormalizedStats<'a, I> {
    pub(crate) stats: I,
    pub(crate) norms: Option<&'a [FrameNorm]>,
    pub(crate) slots: Slots,
}
