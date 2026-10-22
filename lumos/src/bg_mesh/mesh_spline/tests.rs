use imaginarium::Buffer2;

use crate::bg_mesh::BgMesh;
use crate::bg_mesh::mesh_spline::MeshSpline;
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
    let pixels = Buffer2::from_fn(96, 128, |_, y| {
        let k = (y / 32) as f32;
        k * k * C
    });
    let grid = BgMesh::computed(&pixels, None, PARAMS);
    let interior = 2.4 * C / 1024.0;
    let mut spline = MeshSpline::default();
    spline.solve_y(&grid, TileComponent::Sky);
    let sky = spline.d2y.clone();
    spline.solve_y(&grid, TileComponent::Sigma);
    for tx in 0..3 {
        for (ty, expected) in [0.0, interior, interior, 0.0].into_iter().enumerate() {
            let d2 = sky[ty * 3 + tx];
            assert!(
                (d2 - expected).abs() <= 4.0 * f32::EPSILON * interior,
                "sky d″ at ({tx}, {ty}): {d2} vs {expected}"
            );
            assert_eq!(spline.d2y[ty * 3 + tx], 0.0, "σ d″ at ({tx}, {ty})");
        }
    }
}

/// The σ plane is held to the tiles' range and the sky is not: four tile rows of σ 1, 1, 1, 4
/// (rows of ±σ pixels), whose spline overshoots below 1 between the first rows, read back no lower
/// than 1, while a sky rising as `k²` past the last centre runs on above the last tile's.
#[test]
fn only_the_noise_is_held_to_the_tiles_range() {
    let sigmas = [1.0f32, 1.0, 1.0, 4.0];
    let pixels = Buffer2::from_fn(64, 128, |x, y| {
        let k = y / 32;
        let sign = if (x + y) % 2 == 0 { 1.0 } else { -1.0 };
        (k * k) as f32 + sign * sigmas[k]
    });
    let grid = BgMesh::computed(&pixels, None, PARAMS);
    let mut spline = MeshSpline::default();
    let range = grid.sigma_range();
    let mut noise = Buffer2::new_default(64, 128);
    spline.interpolate(&grid, TileComponent::Sigma, &mut noise);
    assert!(
        noise.pixels().iter().all(|sigma| range.contains(sigma)),
        "σ outside {range:?}"
    );
    let mut sky = Buffer2::new_default(64, 128);
    spline.interpolate(&grid, TileComponent::Sky, &mut sky);
    let last_tile = grid.stats[(0, 3)].sky;
    assert!(
        sky[(0, 127)] > last_tile,
        "{} vs {last_tile}",
        sky[(0, 127)]
    );
}
