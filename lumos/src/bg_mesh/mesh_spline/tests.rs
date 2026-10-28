use common::Buffer2;

use crate::bg_mesh::BgMesh;
use crate::bg_mesh::mesh_spline::MeshSpline;
use crate::bg_mesh::mesh_surface::MeshSurface;
use crate::bg_mesh::mesh_workspace::{MedianFilter, MeshParams};
use crate::bg_mesh::tile_stats::TileComponent;

/// Tiles of 32 px, two σ-clip passes and the 3×3 median.
const PARAMS: MeshParams = MeshParams {
    tile_size: 32,
    sigma_clip_max_iterations: 2,
    median_filter: MedianFilter::Apply,
};

/// The y spline's second derivatives through the mesh: four tile rows of skies `k²·c` (c = 1/64)
/// at a uniform 32 px solve, as `solve_d2_quadratic_data` does on unit spacing, to `d″ = [0, 2.4,
/// 2.4, 0] · c/32²`, in every tile column; the flat σ plane has none. The 3×3 median keeps the
/// rows: each window, reflected at the ends, is centred on its own value.
#[test]
fn y_spline_derivatives_through_the_mesh() {
    const C: f32 = 1.0 / 64.0;
    let pixels = Buffer2::from_fn((96, 128), |at| {
        let k = (at.y / 32) as f32;
        k * k * C
    });
    let grid = BgMesh::computed(&pixels, None, PARAMS);
    let interior = 2.4 * C / 1024.0;
    let mut spline = MeshSpline::default();
    let mut sky = MeshSurface::default();
    let mut sigma = MeshSurface::default();
    spline.solve(&grid, TileComponent::Sky, &mut sky);
    spline.solve(&grid, TileComponent::Sigma, &mut sigma);
    for tx in 0..3 {
        for (ty, expected) in [0.0, interior, interior, 0.0].into_iter().enumerate() {
            let d2 = sky.d2y[ty * 3 + tx];
            assert!(
                (d2 - expected).abs() <= 4.0 * f32::EPSILON * interior,
                "sky d″ at ({tx}, {ty}): {d2} vs {expected}"
            );
            assert_eq!(sigma.d2y[ty * 3 + tx], 0.0, "σ d″ at ({tx}, {ty})");
        }
    }
}
