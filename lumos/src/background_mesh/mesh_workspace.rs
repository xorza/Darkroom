//! [`MeshWorkspace`]: the buffers a tile mesh is measured in, kept across frames.

use imaginarium::Buffer2;

use crate::background_mesh::BackgroundMesh;
use crate::background_mesh::tile_stats::{CfaColor, TileInput, TileScratch, TileStats};
use crate::bit_buffer2::BitBuffer2;
use crate::math::size2us::Size2us;
use crate::scratch_pool::ScratchPool;

/// The σ-clip passes of each tile a mesh measures by default, for the sky of a frame and the samples
/// of a gradient alike.
pub(crate) const DEFAULT_SIGMA_CLIP_MAX_ITERATIONS: usize = 3;

/// The buffers a tile mesh is measured in: the grid itself, the median filter's second grid, and
/// each worker's tile scratch.
#[derive(Debug, Default)]
pub(crate) struct MeshWorkspace {
    grid: Option<BackgroundMesh>,
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
        color: Option<CfaColor<'_>>,
        params: MeshParams,
    ) -> &BackgroundMesh {
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
            self.grid = Some(BackgroundMesh::new_uninit(dimensions, tile_size));
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

    use crate::background_mesh::BackgroundMesh;
    use crate::background_mesh::mesh_workspace::{MeshParams, MeshWorkspace};
    use crate::bit_buffer2::BitBuffer2;

    impl BackgroundMesh {
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
mod tests {
    use imaginarium::Buffer2;

    use crate::background_mesh::mesh_workspace::{MedianFilter, MeshParams, MeshWorkspace};
    use crate::background_mesh::tile_stats::MAX_TILE_SAMPLES;
    use crate::bit_buffer2::BitBuffer2;
    use crate::math::size2us::Size2us;

    /// Tiles of `tile_size`, two σ-clip passes, and `median_filter`.
    const fn params(tile_size: usize, median_filter: MedianFilter) -> MeshParams {
        MeshParams {
            tile_size,
            sigma_clip_max_iterations: 2,
            median_filter,
        }
    }

    #[test]
    fn repeated_measures_reuse_mesh_allocations_and_preserve_results() {
        let pixels = Buffer2::new_filled(256, 256, 0.5);
        let mut workspace = MeshWorkspace::default();
        let expected = {
            let grid = workspace.measure(&pixels, None, None, params(64, MedianFilter::Apply));
            grid.stats.pixels().to_vec()
        };
        let first_stats_ptr = workspace.grid.as_ref().unwrap().stats.pixels().as_ptr();
        let filter_ptr = workspace
            .median_filter_scratch
            .as_ref()
            .unwrap()
            .pixels()
            .as_ptr();
        {
            let grid = workspace.measure(&pixels, None, None, params(64, MedianFilter::Apply));
            assert_eq!(grid.stats.pixels().as_ptr(), filter_ptr);
            for (actual, expected) in grid.stats.pixels().iter().zip(expected) {
                assert_eq!(actual.sky, expected.sky);
                assert_eq!(actual.sigma, expected.sigma);
            }
        }
        assert_eq!(
            workspace
                .median_filter_scratch
                .as_ref()
                .unwrap()
                .pixels()
                .as_ptr(),
            first_stats_ptr
        );
    }

    #[test]
    fn clear_releases_all_mesh_allocations() {
        let pixels = Buffer2::new_filled(128, 128, 0.5);
        let mut workspace = MeshWorkspace::default();
        workspace.measure(&pixels, None, None, params(32, MedianFilter::Apply));
        assert!(workspace.grid.is_some());
        assert!(workspace.median_filter_scratch.is_some());
        assert_ne!(workspace.tile_scratch.pooled(), 0);

        workspace.clear();

        assert!(workspace.grid.is_none());
        assert!(workspace.median_filter_scratch.is_none());
        assert_eq!(workspace.tile_scratch.pooled(), 0);
    }

    #[test]
    fn a_measure_rebuilds_the_grid_for_layout_changes() {
        let first_pixels = Buffer2::new_filled(100, 70, 0.5);
        let second_pixels = Buffer2::new_filled(64, 32, 0.25);
        let mut workspace = MeshWorkspace::default();

        // 100 × 70 in 32s: remainders of 4 and 6, under half a tile, join their neighbours.
        let grid = workspace.measure(&first_pixels, None, None, params(32, MedianFilter::Skip));
        assert_eq!(grid.stats.width(), 3);
        assert_eq!(grid.stats.height(), 2);

        let grid = workspace.measure(&first_pixels, None, None, params(50, MedianFilter::Skip));
        assert_eq!(grid.stats.width(), 2);
        assert_eq!(grid.stats.height(), 1);

        let grid = workspace.measure(&second_pixels, None, None, params(16, MedianFilter::Skip));
        assert_eq!(grid.stats.width(), 4);
        assert_eq!(grid.stats.height(), 2);
        for stats in grid.stats.pixels() {
            assert_eq!(stats.sky, 0.25);
            assert_eq!(stats.sigma, 0.0);
        }
    }

    #[test]
    fn a_masked_measure_keeps_per_job_scratch_bounded() {
        let pixels = Buffer2::new_filled(512, 512, 0.5);
        let mask = BitBuffer2::new_filled(Size2us::new(512, 512), false);
        let mut workspace = MeshWorkspace::default();

        workspace.measure(&pixels, Some(&mask), None, params(256, MedianFilter::Skip));
        assert_ne!(workspace.tile_scratch.pooled(), 0);
        assert!(
            workspace
                .tile_scratch
                .all_free(|scratch| { scratch.samples.values.capacity() <= MAX_TILE_SAMPLES }),
            "masked value scratch must retain at most {MAX_TILE_SAMPLES} samples per job"
        );
        assert!(
            workspace
                .tile_scratch
                .all_free(|scratch| { scratch.deviations.capacity() <= MAX_TILE_SAMPLES }),
            "deviation scratch must retain at most {MAX_TILE_SAMPLES} samples per job"
        );
    }
}
