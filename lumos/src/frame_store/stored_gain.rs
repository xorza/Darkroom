//! [`StoredGain`]: a frame's flat gain as the combine reads it.

use std::ops::Range;

use arrayvec::ArrayVec;
use imaginarium::Buffer2;

use crate::frame_store::error::FrameStoreError;
use crate::frame_store::plane_store::PlaneStore;
use crate::frame_store::stored_plane::StoredPlane;
use crate::io::image::flat_gain::{FlatGain, GainGrid, GainRows};

/// Each channel's node grid of a frame's [`FlatGain`], wherever the memory tier put it.
#[derive(Debug)]
pub(crate) struct StoredGain {
    grid: GainGrid,
    planes: ArrayVec<StoredPlane, 3>,
}

impl StoredGain {
    pub(crate) fn from_memory(gain: &FlatGain) -> Self {
        let grid = GainGrid::of(gain.size());
        Self {
            grid,
            planes: gain
                .planes()
                .map(|nodes| {
                    StoredPlane::Memory(Buffer2::new(grid.columns, grid.rows, nodes.to_vec()))
                })
                .collect(),
        }
    }

    /// Write each channel's nodes to `store` and memory-map them back.
    pub(crate) fn spill(store: &impl PlaneStore, gain: &FlatGain) -> Result<Self, FrameStoreError> {
        Ok(Self {
            grid: GainGrid::of(gain.size()),
            planes: gain
                .planes()
                .enumerate()
                .map(|(channel, nodes)| store.store_gain(channel, nodes))
                .collect::<Result<_, _>>()?,
        })
    }

    pub(crate) const fn grid(&self) -> GainGrid {
        self.grid
    }

    pub(crate) const fn channels(&self) -> usize {
        self.planes.len()
    }

    /// The node rows the pixel rows `rows` read.
    pub(crate) fn rows(&self, rows: Range<usize>) -> GainRows<'_> {
        let nodes = self.grid.rows_for(rows);
        let columns = self.grid.columns;
        GainRows::new(
            self.grid,
            nodes.start,
            self.planes
                .iter()
                .map(|plane| plane.chunk(nodes.start * columns, nodes.end * columns))
                .collect(),
        )
    }
}
