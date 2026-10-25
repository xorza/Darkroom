//! [`MeshSpline`]: the y pass of the natural bicubic spline through a mesh's tile centres, as
//! SExtractor and SEP interpolate their background maps; a [`MeshSurface`] reads it out.

use crate::bg_mesh::BgMesh;
use crate::bg_mesh::mesh_surface::MeshSurface;
use crate::bg_mesh::spline::natural_spline::NaturalSpline;
use crate::bg_mesh::tile_stats::TileComponent;

/// The y pass of the natural bicubic spline through a mesh's tile centres: each tile column's
/// second derivatives, solved into the [`MeshSurface`] that reads the spline out. Its scratch is
/// kept across frames.
#[derive(Debug, Default)]
pub(crate) struct MeshSpline {
    /// One tile column's values, second derivatives and solve scratch.
    col: Vec<f32>,
    col_d2: Vec<f32>,
    col_scratch: Vec<f32>,
}

impl MeshSpline {
    /// `component` of `mesh` as the surface it carries to every pixel, into `surface`: the tiles'
    /// values and centres, the y pass's second derivatives, and for the σ the tiles' range.
    pub(crate) fn solve(
        &mut self,
        mesh: &BgMesh,
        component: TileComponent,
        surface: &mut MeshSurface,
    ) {
        let tiles_x = mesh.stats.width();
        let tiles_y = mesh.stats.height();
        surface.values.clear();
        surface
            .values
            .extend(mesh.stats.pixels().iter().map(|tile| tile.get(component)));
        surface.centers_x.clone_from(&mesh.centers_x);
        surface.centers_y.clone_from(&mesh.centers_y);
        surface.size = mesh.size;
        surface.clip = match component {
            TileComponent::Sky => None,
            TileComponent::Sigma => Some(mesh.sigma_range()),
        };
        surface.rows.reshape(tiles_x);
        surface.d2y.clear();
        surface.d2y.resize(tiles_x * tiles_y, 0.0);
        if tiles_y < 2 {
            return;
        }
        self.col.resize(tiles_y, 0.0);
        self.col_d2.resize(tiles_y, 0.0);
        self.col_scratch.resize(tiles_y.saturating_sub(2), 0.0);
        for tx in 0..tiles_x {
            for (ty, value) in self.col.iter_mut().enumerate() {
                *value = surface.values[ty * tiles_x + tx];
            }
            NaturalSpline {
                values: &self.col,
                centers: &mesh.centers_y,
            }
            .second_derivatives(&mut self.col_d2, &mut self.col_scratch);
            for (ty, &d2) in self.col_d2.iter().enumerate() {
                surface.d2y[ty * tiles_x + tx] = d2;
            }
        }
    }

    /// Release the scratch, for frames of another layout.
    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests;
