//! [`StoredGain`]: a frame's flat gain as the combine reads it.

use std::ops::Range;
use std::sync::Arc;

use arrayvec::ArrayVec;
use common::Buffer2;

use crate::frame_store::error::FrameStoreError;
use crate::frame_store::plane_sink::{PlaneRole, PlaneSink};
use crate::frame_store::stored_plane::StoredPlane;
use crate::image::flat_gain::frame_gain::FrameGain;
use crate::image::flat_gain::gain_bins::GainBins;
use crate::image::flat_gain::{FlatGain, GainGrid, GainRows};
use common::Size2us;

/// Each channel's node grid of a frame's [`FrameGain`], wherever it lies: a source gain where the
/// run holds it for every frame that shares it, and a frame's own in RAM on the resident tier or
/// its node planes mapped from files on the spilled one.
#[derive(Debug)]
pub(crate) enum StoredGain {
    Shared(Arc<FlatGain>),
    Own(Box<FlatGain>),
    Mapped(Box<MappedGain>),
}

/// A frame's own gain on the spilled tier: the image size it covers, each channel's node plane,
/// and the bins it carries over from the sensor, which its nodes do not give back.
#[derive(Debug)]
pub(crate) struct MappedGain {
    size: Size2us,
    planes: ArrayVec<StoredPlane, 3>,
    bins: ArrayVec<GainBins, 3>,
}

impl StoredGain {
    /// `gain` as a frame keeps it through `sink`: a shared gain as the handle the run holds, a
    /// frame's own each channel's nodes written and mapped back.
    pub(crate) fn write(
        gain: &FrameGain,
        mut sink: impl PlaneSink,
    ) -> Result<Self, FrameStoreError> {
        let own = match gain {
            FrameGain::Shared(gain) => return Ok(Self::Shared(Arc::clone(gain))),
            FrameGain::Own(gain) => gain,
        };
        Ok(Self::Mapped(Box::new(MappedGain {
            size: own.size(),
            planes: own
                .planes()
                .enumerate()
                .map(|(channel, nodes)| sink.write(PlaneRole::Gain(channel), nodes))
                .collect::<Result<_, _>>()?,
            bins: (0..own.channels())
                .map(|channel| own.bins(channel).clone())
                .collect(),
        })))
    }

    /// The image size the grid covers.
    fn size(&self) -> Size2us {
        match self {
            Self::Shared(gain) => gain.size(),
            Self::Own(gain) => gain.size(),
            Self::Mapped(mapped) => mapped.size,
        }
    }

    pub(crate) fn grid(&self) -> GainGrid {
        GainGrid::of(self.size())
    }

    pub(crate) fn channels(&self) -> usize {
        match self {
            Self::Shared(gain) => gain.channels(),
            Self::Own(gain) => gain.channels(),
            Self::Mapped(mapped) => mapped.planes.len(),
        }
    }

    /// The channels of a grid the frame holds as its own; none for a shared one, which the run
    /// holds once for every frame.
    pub(crate) fn own_channels(&self) -> usize {
        match self {
            Self::Shared(_) => 0,
            Self::Own(_) | Self::Mapped(_) => self.channels(),
        }
    }

    /// Each channel's nodes from `range` of its grid, row-major.
    fn nodes(&self, range: Range<usize>) -> ArrayVec<&[f32], 3> {
        match self {
            Self::Shared(gain) => gain.planes().map(|nodes| &nodes[range.clone()]).collect(),
            Self::Own(gain) => gain.planes().map(|nodes| &nodes[range.clone()]).collect(),
            Self::Mapped(mapped) => mapped
                .planes
                .iter()
                .map(|plane| plane.chunk(range.clone()))
                .collect(),
        }
    }

    /// The node rows the pixel rows `rows` read.
    pub(crate) fn rows(&self, rows: Range<usize>) -> GainRows<'_> {
        let grid = self.grid();
        let nodes = grid.rows_for(rows);
        GainRows::new(
            grid,
            nodes.start,
            self.nodes(nodes.start * grid.cols..nodes.end * grid.cols),
        )
    }

    /// The gain as an image carries it: the handle the run holds, or the frame's own, copied out
    /// of its maps.
    pub(super) fn into_frame_gain(self) -> FrameGain {
        match self {
            Self::Shared(gain) => FrameGain::Shared(gain),
            Self::Own(gain) => FrameGain::Own(gain),
            Self::Mapped(mapped) => {
                let MappedGain { size, planes, bins } = *mapped;
                let grid = GainGrid::of(size);
                let count = grid.nodes();
                FrameGain::Own(Box::new(FlatGain::with_bins(
                    size,
                    planes
                        .iter()
                        .map(|plane| {
                            Buffer2::new((grid.cols, grid.rows), plane.chunk(0..count).to_vec())
                        })
                        .collect(),
                    bins,
                )))
            }
        }
    }
}

/// A gain in RAM, as the resident tier stores it: the handle the run holds, or the frame's own,
/// moved in.
impl From<FrameGain> for StoredGain {
    fn from(gain: FrameGain) -> Self {
        match gain {
            FrameGain::Shared(gain) => Self::Shared(gain),
            FrameGain::Own(gain) => Self::Own(gain),
        }
    }
}

#[cfg(feature = "internals")]
pub(crate) mod internals {
    use crate::frame_store::stored_gain::StoredGain;
    use crate::frame_store::stored_plane::StoredPlane;

    impl StoredGain {
        /// The gain's mapped node planes, one per channel; none for a gain in RAM.
        pub(crate) fn planes(&self) -> impl Iterator<Item = &StoredPlane> {
            let planes = match self {
                Self::Shared(_) | Self::Own(_) => None,
                Self::Mapped(mapped) => Some(&mapped.planes),
            };
            planes.into_iter().flatten()
        }
    }
}
