//! Working memory the background estimator keeps between detections.

use crate::background_mesh::workspace::MeshWorkspace;
use crate::scratch_pool::{Scratch, ScratchPool};

#[derive(Debug, Default)]
pub(crate) struct BackgroundWorkspace {
    pub(crate) mesh: MeshWorkspace,
    pub(crate) interpolation: ScratchPool<InterpolateScratch>,
}

impl BackgroundWorkspace {
    pub(crate) fn clear(&mut self) {
        self.mesh.clear();
        self.interpolation.clear();
    }
}

#[derive(Debug, Default)]
pub(crate) struct InterpolateScratch {
    pub(crate) node_bg: Vec<f32>,
    pub(crate) node_noise: Vec<f32>,
    pub(crate) d2x_bg: Vec<f32>,
    pub(crate) d2x_noise: Vec<f32>,
    pub(crate) spline_scratch: Vec<f32>,
    /// The sky row of an interpolation that keeps only the noise.
    pub(crate) discarded_row: Vec<f32>,
}

/// What an [`InterpolateScratch`] is sized for: the mesh's tile columns, and the image's row.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct InterpolateShape {
    pub(crate) tiles_x: usize,
    pub(crate) width: usize,
}

impl Scratch for InterpolateScratch {
    type Shape = InterpolateShape;

    fn build(shape: InterpolateShape) -> Self {
        let mut scratch = Self::default();
        scratch.fit(shape);
        scratch
    }

    fn fit(&mut self, InterpolateShape { tiles_x, width }: InterpolateShape) {
        self.node_bg.resize(tiles_x, 0.0);
        self.node_noise.resize(tiles_x, 0.0);
        self.d2x_bg.resize(tiles_x, 0.0);
        self.d2x_noise.resize(tiles_x, 0.0);
        self.spline_scratch.resize(tiles_x.saturating_sub(2), 0.0);
        self.discarded_row.resize(width, 0.0);
    }

    fn has_shape(&self, InterpolateShape { tiles_x, width }: InterpolateShape) -> bool {
        [
            self.node_bg.len(),
            self.node_noise.len(),
            self.d2x_bg.len(),
            self.d2x_noise.len(),
            self.spline_scratch.len(),
            self.discarded_row.len(),
        ] == [
            tiles_x,
            tiles_x,
            tiles_x,
            tiles_x,
            tiles_x.saturating_sub(2),
            width,
        ]
    }
}

#[cfg(test)]
mod tests {
    use crate::scratch_pool::Scratch;
    use crate::star_detection::background::workspace::{
        BackgroundWorkspace, InterpolateScratch, InterpolateShape,
    };

    /// A lease is sized to the pool's shape whichever way it came — built, or reused from a larger
    /// one — and `clear` empties the pool.
    #[test]
    fn interpolation_scratch_fits_its_shape_and_is_released_by_clear() {
        let shape = |tiles_x, width| InterpolateShape { tiles_x, width };
        let mut workspace = BackgroundWorkspace::default();
        workspace.interpolation.reshape(shape(9, 40));
        let built = workspace.interpolation.acquire();
        assert_eq!(built.node_bg.len(), 9);
        assert_eq!(built.spline_scratch.len(), 7);
        assert_eq!(built.discarded_row.len(), 40);
        drop(built);
        assert_eq!(workspace.interpolation.pooled(), 1);

        let mut reused = InterpolateScratch::build(shape(9, 40));
        reused.fit(shape(4, 16));
        assert_eq!(
            [
                reused.node_bg.len(),
                reused.d2x_noise.len(),
                reused.spline_scratch.len()
            ],
            [4, 4, 2]
        );
        assert!(reused.has_shape(shape(4, 16)));
        assert!(!reused.has_shape(shape(4, 40)));

        workspace.clear();
        assert_eq!(workspace.interpolation.pooled(), 0);
    }
}
