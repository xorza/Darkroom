//! Working memory the background estimator keeps between detections.

use crate::bg_mesh::mesh_spline::MeshSpline;
use crate::bg_mesh::mesh_workspace::MeshWorkspace;

/// The tile mesh, its spline, and the tile σ a frame's noise floor is the median of.
#[derive(Debug, Default)]
pub(crate) struct BgWorkspace {
    pub(crate) mesh: MeshWorkspace,
    pub(crate) spline: MeshSpline,
    pub(crate) values: Vec<f32>,
}

impl BgWorkspace {
    pub(crate) fn clear(&mut self) {
        self.mesh.clear();
        self.spline.clear();
    }
}
