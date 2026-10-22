//! [`StoredGain`]: a frame's flat gain as the combine reads it.

use std::ops::Range;
use std::sync::Arc;

use arrayvec::ArrayVec;
use imaginarium::Buffer2;

use crate::frame_store::error::FrameStoreError;
use crate::frame_store::run_scratch::RunScratch;
use crate::frame_store::stored_plane::StoredPlane;
use crate::image::flat_gain::{FlatGain, GainGrid, GainRows};
use crate::math::size2us::Size2us;

/// Each channel's node grid of a frame's [`FlatGain`], wherever the memory tier put it: the image's
/// own gain on the resident tier, node planes mapped from files on the spilled one.
#[derive(Debug)]
pub(crate) enum StoredGain {
    Resident(Arc<FlatGain>),
    Mapped {
        size: Size2us,
        planes: ArrayVec<StoredPlane, 3>,
    },
}

impl StoredGain {
    /// Write each channel's nodes to `scratch` and memory-map them back.
    pub(crate) fn spill(scratch: &RunScratch, gain: &FlatGain) -> Result<Self, FrameStoreError> {
        Ok(Self::Mapped {
            size: gain.size(),
            planes: gain
                .planes()
                .map(|nodes| scratch.store(nodes))
                .collect::<Result<_, _>>()?,
        })
    }

    /// The image size the grid covers.
    fn size(&self) -> Size2us {
        match self {
            Self::Resident(gain) => gain.size(),
            Self::Mapped { size, .. } => *size,
        }
    }

    pub(crate) fn grid(&self) -> GainGrid {
        GainGrid::of(self.size())
    }

    pub(crate) fn channels(&self) -> usize {
        match self {
            Self::Resident(gain) => gain.channels(),
            Self::Mapped { planes, .. } => planes.len(),
        }
    }

    /// Each channel's nodes from `range` of its grid, row-major.
    fn nodes(&self, range: Range<usize>) -> ArrayVec<&[f32], 3> {
        match self {
            Self::Resident(gain) => gain.planes().map(|nodes| &nodes[range.clone()]).collect(),
            Self::Mapped { planes, .. } => planes
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

    /// The gain as a [`FlatGain`] in RAM: shared when it is one already, copied out of its maps
    /// otherwise.
    pub(super) fn to_flat_gain(&self) -> Arc<FlatGain> {
        match self {
            Self::Resident(gain) => Arc::clone(gain),
            Self::Mapped { size, planes } => {
                let grid = GainGrid::of(*size);
                let count = grid.cols * grid.rows;
                Arc::new(FlatGain::from_nodes(
                    *size,
                    planes
                        .iter()
                        .map(|plane| {
                            Buffer2::new(grid.cols, grid.rows, plane.chunk(0..count).to_vec())
                        })
                        .collect(),
                ))
            }
        }
    }
}

#[cfg(feature = "internals")]
pub(crate) mod internals {
    use crate::frame_store::stored_gain::StoredGain;
    use crate::frame_store::stored_plane::StoredPlane;

    impl StoredGain {
        /// The gain's mapped node planes, one per channel; none for a resident gain.
        pub(crate) fn planes(&self) -> impl Iterator<Item = &StoredPlane> {
            let planes = match self {
                Self::Resident(_) => None,
                Self::Mapped { planes, .. } => Some(planes),
            };
            planes.into_iter().flatten()
        }
    }
}
