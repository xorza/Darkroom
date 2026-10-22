//! [`FrameGain`]: the flat gain an image carries, and who holds its grid.

use std::ops::Deref;
use std::sync::Arc;

use crate::image::flat_gain::FlatGain;

/// The flat gain an image carries: a calibration product the frames of a run share, or a grid made
/// for this frame alone. What made the gain says which, so a store and a plan know what the frame
/// holds of its own without asking who else holds the grid.
#[derive(Debug, Clone)]
pub enum FrameGain {
    /// A source gain: the masters' flat's, or one a file stored, held once for every frame equal
    /// to it.
    Shared(Arc<FlatGain>),
    /// The frame's source gain read through its warp or its drizzle.
    Own(Box<FlatGain>),
}

impl Deref for FrameGain {
    type Target = FlatGain;

    fn deref(&self) -> &FlatGain {
        match self {
            Self::Shared(gain) => gain,
            Self::Own(gain) => gain,
        }
    }
}
