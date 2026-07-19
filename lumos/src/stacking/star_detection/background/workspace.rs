use crate::background_mesh::workspace::MeshWorkspace;
use crate::concurrency::JobScratchPool;

#[derive(Debug, Default)]
pub(crate) struct BackgroundWorkspace {
    pub(crate) mesh: MeshWorkspace,
    pub(crate) interpolation: JobScratchPool<InterpolateScratch>,
}

impl BackgroundWorkspace {
    pub(crate) fn clear(&mut self) {
        self.mesh.clear();
        self.interpolation = JobScratchPool::default();
    }
}

#[derive(Debug, Default)]
pub(crate) struct InterpolateScratch {
    pub(crate) node_bg: Vec<f32>,
    pub(crate) node_noise: Vec<f32>,
    pub(crate) d2x_bg: Vec<f32>,
    pub(crate) d2x_noise: Vec<f32>,
    pub(crate) spline_scratch: Vec<f32>,
}

impl InterpolateScratch {
    pub(crate) fn resize(&mut self, tiles_x: usize) {
        self.node_bg.resize(tiles_x, 0.0);
        self.node_noise.resize(tiles_x, 0.0);
        self.d2x_bg.resize(tiles_x, 0.0);
        self.d2x_noise.resize(tiles_x, 0.0);
        self.spline_scratch.resize(tiles_x.saturating_sub(2), 0.0);
    }
}

#[cfg(test)]
mod tests {
    use crate::concurrency::test_support::job_count;
    use crate::stacking::star_detection::background::workspace::BackgroundWorkspace;

    #[test]
    fn interpolation_scratch_resizes_and_is_released_by_clear() {
        let mut workspace = BackgroundWorkspace::default();
        {
            let mut scratch = workspace.interpolation.acquire();
            scratch.resize(4);
            scratch.resize(9);
            assert_eq!(scratch.node_bg.len(), 9);
            assert_eq!(scratch.spline_scratch.len(), 7);
        }
        assert_eq!(job_count(&workspace.interpolation), 1);

        workspace.clear();
        assert_eq!(job_count(&workspace.interpolation), 0);
    }
}
