use crate::background_mesh::TileGrid;
use crate::background_mesh::TileStats;
use crate::concurrency::JobScratchPool;
use common::BitBuffer2;
use imaginarium::Buffer2;

#[derive(Debug, Default)]
pub(crate) struct TileScratch {
    pub(crate) values: Vec<f32>,
    pub(crate) deviations: Vec<f32>,
}

#[derive(Debug, Default)]
pub(crate) struct MeshWorkspace {
    grid: Option<TileGrid>,
    median_filter_scratch: Option<Buffer2<TileStats>>,
    spline_values: Vec<f32>,
    spline_d2: Vec<f32>,
    spline_scratch: Vec<f32>,
    tile_scratch: JobScratchPool<TileScratch>,
}

impl MeshWorkspace {
    pub(crate) fn compute(
        &mut self,
        pixels: &Buffer2<f32>,
        mask: Option<&BitBuffer2>,
        tile_size: usize,
        sigma_clip_iterations: usize,
        median_filter: bool,
    ) -> &TileGrid {
        let width = pixels.width();
        let height = pixels.height();
        if self
            .grid
            .as_ref()
            .is_none_or(|grid| !grid.matches_layout(width, height, tile_size))
        {
            self.grid = Some(TileGrid::new_uninit(width, height, tile_size));
            self.median_filter_scratch = None;
        }

        let grid = self.grid.as_ref().unwrap();
        let tiles_x = grid.stats.width();
        let tiles_y = grid.stats.height();
        if median_filter
            && self
                .median_filter_scratch
                .as_ref()
                .is_none_or(|scratch| scratch.width() != tiles_x || scratch.height() != tiles_y)
        {
            self.median_filter_scratch = Some(Buffer2::new_default(tiles_x, tiles_y));
        }
        self.spline_values.resize(tiles_y, 0.0);
        self.spline_d2.resize(tiles_y, 0.0);
        self.spline_scratch.resize(tiles_y.saturating_sub(2), 0.0);
        let grid = self.grid.as_mut().unwrap();
        grid.fill_tile_stats(pixels, mask, sigma_clip_iterations, &self.tile_scratch);
        if median_filter {
            grid.apply_median_filter(self.median_filter_scratch.as_mut().unwrap());
        }
        grid.compute_y_spline_derivatives(
            &mut self.spline_values,
            &mut self.spline_d2,
            &mut self.spline_scratch,
        );
        grid
    }

    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use crate::background_mesh::TileGrid;
    use crate::background_mesh::workspace::MeshWorkspace;
    use common::BitBuffer2;
    use imaginarium::Buffer2;

    pub(crate) fn compute_grid(
        pixels: &Buffer2<f32>,
        mask: Option<&BitBuffer2>,
        tile_size: usize,
        sigma_clip_iterations: usize,
        median_filter: bool,
    ) -> TileGrid {
        let mut workspace = MeshWorkspace::default();
        workspace.compute(
            pixels,
            mask,
            tile_size,
            sigma_clip_iterations,
            median_filter,
        );
        workspace.grid.take().unwrap()
    }
}

#[cfg(test)]
mod tests {
    use crate::background_mesh::workspace::MeshWorkspace;
    use crate::concurrency::test_support::job_count;
    use imaginarium::Buffer2;

    const SIGMA_CLIP_ITERATIONS: usize = 2;

    #[test]
    fn repeated_compute_reuses_mesh_allocations_and_preserves_results() {
        let pixels = Buffer2::new_filled(256, 256, 0.5);
        let mut workspace = MeshWorkspace::default();
        let expected = {
            let grid = workspace.compute(&pixels, None, 64, SIGMA_CLIP_ITERATIONS, true);
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
        let tile_job_count = job_count(&workspace.tile_scratch);

        {
            let grid = workspace.compute(&pixels, None, 64, SIGMA_CLIP_ITERATIONS, true);
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
        assert_eq!(job_count(&workspace.tile_scratch), tile_job_count);
    }

    #[test]
    fn clear_releases_all_mesh_allocations() {
        let pixels = Buffer2::new_filled(128, 128, 0.5);
        let mut workspace = MeshWorkspace::default();
        workspace.compute(&pixels, None, 32, SIGMA_CLIP_ITERATIONS, true);
        assert!(workspace.grid.is_some());
        assert!(workspace.median_filter_scratch.is_some());
        assert_ne!(job_count(&workspace.tile_scratch), 0);

        workspace.clear();

        assert!(workspace.grid.is_none());
        assert!(workspace.median_filter_scratch.is_none());
        assert!(workspace.spline_values.is_empty());
        assert!(workspace.spline_d2.is_empty());
        assert!(workspace.spline_scratch.is_empty());
        assert_eq!(job_count(&workspace.tile_scratch), 0);
    }

    #[test]
    fn compute_rebuilds_grid_for_layout_changes() {
        let first_pixels = Buffer2::new_filled(100, 70, 0.5);
        let second_pixels = Buffer2::new_filled(64, 32, 0.25);
        let mut workspace = MeshWorkspace::default();

        let grid = workspace.compute(&first_pixels, None, 32, SIGMA_CLIP_ITERATIONS, false);
        assert_eq!(grid.stats.width(), 4);
        assert_eq!(grid.stats.height(), 3);

        let grid = workspace.compute(&first_pixels, None, 50, SIGMA_CLIP_ITERATIONS, false);
        assert_eq!(grid.stats.width(), 2);
        assert_eq!(grid.stats.height(), 2);

        let grid = workspace.compute(&second_pixels, None, 16, SIGMA_CLIP_ITERATIONS, false);
        assert_eq!(grid.stats.width(), 4);
        assert_eq!(grid.stats.height(), 2);
        for stats in grid.stats.pixels() {
            assert_eq!(stats.sky, 0.25);
            assert_eq!(stats.sigma, 0.0);
        }
    }
}
