//! [`MeshWorkspace`]: the buffers a tile mesh is measured in, kept across frames.

use imaginarium::Buffer2;

use crate::background_mesh::BackgroundMesh;
use crate::background_mesh::tile_stats::{Eligible, TileInput, TileScratch, TileStats};
use crate::bit_buffer2::BitBuffer2;
use crate::math::size2us::Size2us;
use crate::scratch_pool::ScratchPool;

/// The buffers a tile mesh is measured in: the grid itself, the median filter's second grid, the
/// spline solve's columns, and each worker's tile scratch.
#[derive(Debug, Default)]
pub(crate) struct MeshWorkspace {
    grid: Option<BackgroundMesh>,
    median_filter_scratch: Option<Buffer2<TileStats>>,
    spline_values: Vec<f32>,
    spline_d2: Vec<f32>,
    spline_scratch: Vec<f32>,
    tile_scratch: ScratchPool<TileScratch>,
}

/// How a mesh cuts and measures its tiles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MeshParams {
    pub(crate) tile_size: usize,
    /// The σ-clip passes of each tile.
    pub(crate) sigma_clip_iterations: usize,
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
    /// The tile grid of `pixels` with its statistics and the y-spline derivatives that
    /// interpolating it reads.
    pub(crate) fn compute(
        &mut self,
        pixels: &Buffer2<f32>,
        mask: Option<&BitBuffer2>,
        params: MeshParams,
    ) -> &BackgroundMesh {
        let input = TileInput {
            pixels,
            mask,
            eligible: None,
            sigma_clip_iterations: params.sigma_clip_iterations,
        };
        self.fill_tile_stats(&input, params);
        let tiles_y = self.grid.as_ref().unwrap().stats.height();
        self.spline_values.resize(tiles_y, 0.0);
        self.spline_d2.resize(tiles_y, 0.0);
        self.spline_scratch.resize(tiles_y.saturating_sub(2), 0.0);
        let grid = self.grid.as_mut().unwrap();
        grid.compute_y_spline_derivatives(
            &mut self.spline_values,
            &mut self.spline_d2,
            &mut self.spline_scratch,
        );
        grid
    }

    /// The tile grid of `pixels` with its statistics only, for a caller that reads the tiles and
    /// never interpolates between them.
    pub(crate) fn tile_stats(
        &mut self,
        pixels: &Buffer2<f32>,
        mask: Option<&BitBuffer2>,
        params: MeshParams,
    ) -> &BackgroundMesh {
        let input = TileInput {
            pixels,
            mask,
            eligible: None,
            sigma_clip_iterations: params.sigma_clip_iterations,
        };
        self.fill_tile_stats(&input, params);
        self.grid.as_ref().unwrap()
    }

    /// [`Self::tile_stats`] over the pixels `eligible` takes alone, such as one colour of a mosaic.
    pub(super) fn tile_stats_where(
        &mut self,
        pixels: &Buffer2<f32>,
        eligible: &Eligible<'_>,
        params: MeshParams,
    ) -> &BackgroundMesh {
        let input = TileInput {
            pixels,
            mask: None,
            eligible: Some(eligible),
            sigma_clip_iterations: params.sigma_clip_iterations,
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
        /// The mesh [`MeshWorkspace::compute`] measures, in a workspace of its own.
        pub(crate) fn computed(
            pixels: &Buffer2<f32>,
            mask: Option<&BitBuffer2>,
            params: MeshParams,
        ) -> Self {
            let mut workspace = MeshWorkspace::default();
            workspace.compute(pixels, mask, params);
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
            sigma_clip_iterations: 2,
            median_filter,
        }
    }

    #[test]
    fn repeated_compute_reuses_mesh_allocations_and_preserves_results() {
        let pixels = Buffer2::new_filled(256, 256, 0.5);
        let mut workspace = MeshWorkspace::default();
        let expected = {
            let grid = workspace.compute(&pixels, None, params(64, MedianFilter::Apply));
            grid.stats.pixels().to_vec()
        };
        let first_stats_ptr = workspace.grid.as_ref().unwrap().stats.pixels().as_ptr();
        let filter_ptr = workspace
            .median_filter_scratch
            .as_ref()
            .unwrap()
            .pixels()
            .as_ptr();
        let spline_values_ptr = workspace.spline_values.as_ptr();
        let spline_d2_ptr = workspace.spline_d2.as_ptr();
        let spline_scratch_ptr = workspace.spline_scratch.as_ptr();
        {
            let grid = workspace.compute(&pixels, None, params(64, MedianFilter::Apply));
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
        assert_eq!(workspace.spline_values.as_ptr(), spline_values_ptr);
        assert_eq!(workspace.spline_d2.as_ptr(), spline_d2_ptr);
        assert_eq!(workspace.spline_scratch.as_ptr(), spline_scratch_ptr);
    }

    #[test]
    fn clear_releases_all_mesh_allocations() {
        let pixels = Buffer2::new_filled(128, 128, 0.5);
        let mut workspace = MeshWorkspace::default();
        workspace.compute(&pixels, None, params(32, MedianFilter::Apply));
        assert!(workspace.grid.is_some());
        assert!(workspace.median_filter_scratch.is_some());
        assert_ne!(workspace.tile_scratch.pooled(), 0);

        workspace.clear();

        assert!(workspace.grid.is_none());
        assert!(workspace.median_filter_scratch.is_none());
        assert!(workspace.spline_values.is_empty());
        assert!(workspace.spline_d2.is_empty());
        assert!(workspace.spline_scratch.is_empty());
        assert_eq!(workspace.tile_scratch.pooled(), 0);
    }

    #[test]
    fn compute_rebuilds_grid_for_layout_changes() {
        let first_pixels = Buffer2::new_filled(100, 70, 0.5);
        let second_pixels = Buffer2::new_filled(64, 32, 0.25);
        let mut workspace = MeshWorkspace::default();

        // 100 × 70 in 32s: remainders of 4 and 6, under half a tile, join their neighbours.
        let grid = workspace.compute(&first_pixels, None, params(32, MedianFilter::Skip));
        assert_eq!(grid.stats.width(), 3);
        assert_eq!(grid.stats.height(), 2);

        let grid = workspace.compute(&first_pixels, None, params(50, MedianFilter::Skip));
        assert_eq!(grid.stats.width(), 2);
        assert_eq!(grid.stats.height(), 1);

        let grid = workspace.compute(&second_pixels, None, params(16, MedianFilter::Skip));
        assert_eq!(grid.stats.width(), 4);
        assert_eq!(grid.stats.height(), 2);
        for stats in grid.stats.pixels() {
            assert_eq!(stats.sky, 0.25);
            assert_eq!(stats.sigma, 0.0);
        }
    }

    #[test]
    fn masked_compute_keeps_per_job_scratch_bounded() {
        let pixels = Buffer2::new_filled(512, 512, 0.5);
        let mask = BitBuffer2::new_filled(Size2us::new(512, 512), false);
        let mut workspace = MeshWorkspace::default();

        workspace.compute(&pixels, Some(&mask), params(256, MedianFilter::Skip));
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
