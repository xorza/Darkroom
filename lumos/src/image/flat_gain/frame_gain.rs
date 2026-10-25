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
    /// A grid this frame alone holds: its source gain read through its warp or its drizzle, or a
    /// gain read back with the frame from a cache of its own.
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

impl FrameGain {
    /// Resident RAM the gain adds to the image that carries it: the grid of a gain it holds
    /// alone. A shared gain's owner counts it.
    pub fn own_ram_bytes(&self) -> usize {
        match self {
            Self::Own(gain) => gain.ram_bytes(),
            Self::Shared(_) => 0,
        }
    }
}
