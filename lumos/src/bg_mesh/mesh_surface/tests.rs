use common::Buffer2;

use crate::bg_mesh::BgMesh;
use crate::bg_mesh::mesh_spline::MeshSpline;
use crate::bg_mesh::mesh_surface::MeshSurface;
use crate::bg_mesh::mesh_workspace::{MedianFilter, MeshParams};
use crate::bg_mesh::tile_stats::TileComponent;
use common::Vec2us;

/// Tiles of 32 px, two σ-clip passes and the 3×3 median.
const PARAMS: MeshParams = MeshParams {
    tile_size: 32,
    sigma_clip_max_iterations: 2,
    median_filter: MedianFilter::Apply,
};

/// The σ plane is held to the tiles' range and the sky is not: four tile rows of σ 1, 1, 1, 4
/// (rows of ±σ pixels), whose spline overshoots below 1 between the first rows, read back no lower
/// than 1, while a sky rising as `k²` past the last centre runs on above the last tile's.
#[test]
fn only_the_noise_is_held_to_the_tiles_range() {
    let sigmas = [1.0f32, 1.0, 1.0, 4.0];
    let pixels = Buffer2::from_fn((64, 128), |x, y| {
        let k = y / 32;
        let sign = if (x + y) % 2 == 0 { 1.0 } else { -1.0 };
        (k * k) as f32 + sign * sigmas[k]
    });
    let grid = BgMesh::computed(&pixels, None, PARAMS);
    let mut spline = MeshSpline::default();
    let range = grid.sigma_range();
    let mut surface = MeshSurface::default();
    spline.solve(&grid, TileComponent::Sigma, &mut surface);
    let noise = surface.plane();
    assert!(
        noise.pixels().iter().all(|sigma| range.contains(sigma)),
        "σ outside {range:?}"
    );
    spline.solve(&grid, TileComponent::Sky, &mut surface);
    let sky = surface.plane();
    let last_tile = grid.stats[(0, 3)].sky;
    assert!(
        sky[(0, 127)] > last_tile,
        "{} vs {last_tile}",
        sky[(0, 127)]
    );
}

/// A pixel read alone is the value its row holds there, bit for bit, the σ's clip included: every
/// pixel of a 100 × 70 frame of 32 px tiles, a sky of a tilted ramp and a σ that varies by tile,
/// on whichever vector tier the row runs.
#[test]
fn a_pixel_is_its_row_bit_for_bit() {
    let pixels = Buffer2::from_fn((100, 70), |x, y| {
        let sign = if (x + y) % 2 == 0 { 1.0 } else { -1.0 };
        0.1 + 0.001 * x as f32 + 0.002 * y as f32 + sign * (0.01 + 0.0005 * (x / 32) as f32)
    });
    let grid = BgMesh::computed(&pixels, None, PARAMS);
    let mut spline = MeshSpline::default();
    for component in [TileComponent::Sky, TileComponent::Sigma] {
        let mut surface = MeshSurface::default();
        spline.solve(&grid, component, &mut surface);
        let plane = surface.plane();
        let mut reader = surface.reader();
        for y in 0..70 {
            for x in 0..100 {
                assert_eq!(
                    reader.at(Vec2us::new(x, y)).to_bits(),
                    plane[(x, y)].to_bits(),
                    "{component:?} at ({x}, {y})"
                );
            }
        }
    }
}

/// A row's lower tile row is the last whose centre is at or before it, and the first before the
/// first centre: 32-px tiles over 128 rows centre at 15.5, 47.5, 79.5, 111.5; one tile is always
/// the first.
#[test]
fn the_lower_tile_row_is_the_last_centre_at_or_before() {
    let mut spline = MeshSpline::default();
    let mut surface = MeshSurface::default();
    let grid = BgMesh::computed(&Buffer2::new_filled((64, 128), 0.5), None, PARAMS);
    spline.solve(&grid, TileComponent::Sky, &mut surface);
    for (pos, tile) in [
        (-10.0, 0),
        (0.0, 0),
        (15.5, 0),
        (47.4, 0),
        (47.5, 1),
        (79.5, 2),
        (111.4, 2),
        (111.5, 3),
        (1000.0, 3),
    ] {
        assert_eq!(surface.lower_tile_row(pos), tile, "y = {pos}");
    }
    let single = BgMesh::computed(&Buffer2::new_filled((32, 32), 0.5), None, PARAMS);
    spline.solve(&single, TileComponent::Sky, &mut surface);
    for pos in [0.0, 15.5, 100.0] {
        assert_eq!(surface.lower_tile_row(pos), 0, "one tile, y = {pos}");
    }
}
