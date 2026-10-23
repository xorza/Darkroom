//! [`MeshWorkspace`]: the buffers a tile mesh is measured in, kept across frames.

use imaginarium::Buffer2;

use crate::bg_mesh::BgMesh;
use crate::bg_mesh::tile_stats::{ColorSites, TileInput, TileScratch, TileStats};
use crate::bit_buffer2::BitBuffer2;
use crate::math::size2us::Size2us;
use crate::scratch_pool::ScratchPool;

/// The σ-clip passes of each tile a mesh measures by default, for the sky of a frame and the
/// samples of a gradient alike.
pub(crate) const DEFAULT_SIGMA_CLIP_MAX_ITERATIONS: usize = 3;

/// The buffers a tile mesh is measured in: the grid itself, the median filter's second grid, and
/// each worker's tile scratch.
#[derive(Debug, Default)]
pub(crate) struct MeshWorkspace {
    grid: Option<BgMesh>,
    median_filter_scratch: Option<Buffer2<TileStats>>,
    tile_scratch: ScratchPool<TileScratch>,
}

/// How a mesh cuts and measures its tiles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MeshParams {
    pub(crate) tile_size: usize,
    /// The σ-clip passes of each tile.
    pub(crate) sigma_clip_max_iterations: usize,
    pub(crate) median_filter: MedianFilter,
}

/// Whether each tile takes the median of its 3×3 neighbourhood, which keeps a tile a bright object
/// spoiled from reaching the background.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MedianFilter {
    Apply,
    Skip,
}

impl MeshWorkspace {
    /// The tile grid of `pixels` with its statistics, leaving out the pixels `mask` sets: of the
    /// photosites of `color` alone when given.
    pub(crate) fn measure(
        &mut self,
        pixels: &Buffer2<f32>,
        mask: Option<&BitBuffer2>,
        color: Option<ColorSites>,
        params: MeshParams,
    ) -> &BgMesh {
        let input = TileInput {
            pixels,
            mask,
            color,
            sigma_clip_max_iterations: params.sigma_clip_max_iterations,
        };
        self.fill_tile_stats(&input, params);
        self.grid.as_ref().unwrap()
    }

    /// Measure every tile of `input` into the grid, rebuilding the grid when the frame or the tile
    /// size changed.
    fn fill_tile_stats(&mut self, input: &TileInput<'_>, params: MeshParams) {
        let dimensions = Size2us::from(input.pixels);
        let tile_size = params.tile_size;
        if self
            .grid
            .as_ref()
            .is_none_or(|grid| !grid.matches_layout(dimensions, tile_size))
        {
            self.grid = Some(BgMesh::new_uninit(dimensions, tile_size));
            self.median_filter_scratch = None;
        }

        let grid = self.grid.as_ref().unwrap();
        let tiles_x = grid.stats.width();
        let tiles_y = grid.stats.height();
        let median_filter = params.median_filter == MedianFilter::Apply;
        if median_filter
            && self
                .median_filter_scratch
                .as_ref()
                .is_none_or(|scratch| scratch.width() != tiles_x || scratch.height() != tiles_y)
        {
            self.median_filter_scratch = Some(Buffer2::new_default(tiles_x, tiles_y));
        }
        let grid = self.grid.as_mut().unwrap();
        grid.fill_tile_stats(input, &self.tile_scratch);
        if median_filter {
            grid.apply_median_filter(self.median_filter_scratch.as_mut().unwrap());
        }
    }

    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
pub(crate) mod internals {
    use imaginarium::Buffer2;

    use crate::bg_mesh::BgMesh;
    use crate::bg_mesh::mesh_workspace::{MeshParams, MeshWorkspace};
    use crate::bit_buffer2::BitBuffer2;

    impl BgMesh {
        /// The mesh [`MeshWorkspace::measure`] measures, in a workspace of its own.
        pub(crate) fn computed(
            pixels: &Buffer2<f32>,
            mask: Option<&BitBuffer2>,
            params: MeshParams,
        ) -> Self {
            let mut workspace = MeshWorkspace::default();
            workspace.measure(pixels, mask, None, params);
            workspace.grid.take().unwrap()
        }
    }
}

#[cfg(test)]
mod tests;
