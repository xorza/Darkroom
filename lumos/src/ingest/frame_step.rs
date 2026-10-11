//! [`FrameStep`]: what a decoded frame goes through before its checks and statistics.

use std::fmt::Debug;

/// A step each decoded frame of a run takes before its checks and statistics, given the frame's
/// index: the subtraction of a calibration master, for one. A frame through a step is not what its
/// source decodes to, so it is never committed to a kept cache.
pub(crate) trait FrameStep<I>: Sync + Debug {
    /// Why the step could not be taken on a frame.
    type Error;

    fn apply(&self, index: usize, image: &mut I) -> Result<(), Self::Error>;
}
