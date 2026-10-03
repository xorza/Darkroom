use crate::background_mesh::workspace::internals::compute_grid;
use crate::background_mesh::*;
use crate::math::statistics::mad_to_sigma;

/// Number of sigma-clipping iterations for tests.
const TEST_SIGMA_CLIP_ITERATIONS: usize = 2;

/// Create a `TileGrid` with default test parameters (no mask, default sigma clip iterations)
fn make_grid(pixels: &Buffer2<f32>, tile_size: usize) -> TileGrid {
    compute_grid(pixels, None, tile_size, TEST_SIGMA_CLIP_ITERATIONS, true)
}

/// Create a `TileGrid` with mask
fn make_grid_with_mask(pixels: &Buffer2<f32>, tile_size: usize, mask: &BitBuffer2) -> TileGrid {
    compute_grid(
        pixels,
        Some(mask),
        tile_size,
        TEST_SIGMA_CLIP_ITERATIONS,
        true,
    )
}

#[test]
#[should_panic(expected = "non-zero")]
fn new_uninit_zero_dimension_panics() {
    // A zero-size image is a logic error upstream (ImageDimensions asserts > 0); fail fast
    // with a clear message instead of a bare div_ceil divide-by-zero.
    TileGrid::new_uninit(Size2us::new(0, 100), 64);
}

/// The grid's shape, and a flat image's statistics on it. A tile size past the image clamps to
/// its shorter side — 10 × 8 at 64 takes 8, whose remainder of 2 joins the one tile, never a 0-tile
/// grid — and a remainder under half a tile joins the tile before it, as 100 × 70 at 32 shows. A
/// flat tile has no spread: σ = 0 sends the sky to the median, which is the value itself, negative
/// or not.
#[test]
fn grid_shape_and_flat_skies() {
    for (size, tile_size, columns, rows, value) in [
        (Size2us::new(128, 64), 32, 4, 2, 0.3f32),
        (Size2us::new(100, 70), 32, 3, 2, 0.5),
        (Size2us::new(100, 50), 200, 2, 1, 0.3),
        (Size2us::new(1000, 10), 64, 100, 1, 0.5),
        (Size2us::new(10, 1000), 64, 1, 100, 0.5),
        (Size2us::new(10, 8), 64, 1, 1, 0.5),
        (Size2us::new(64, 64), 32, 2, 2, -0.5),
        (Size2us::new(64, 64), 64, 1, 1, 100.0),
    ] {
        let grid = make_grid(
            &Buffer2::new_filled(size.width, size.height, value),
            tile_size,
        );
        assert_eq!(
            (grid.stats.width(), grid.stats.height()),
            (columns, rows),
            "{size:?} at {tile_size}"
        );
        for stats in grid.stats.pixels() {
            assert_eq!(
                (stats.sky, stats.sigma),
                (value, 0.0),
                "{size:?} at {tile_size}"
            );
        }
    }
}

/// A bright-ward tail that survives clipping pulls the mean above the median, and the Pearson mode
/// `2.5·median − 1.5·mean` takes the sky below both. One 32 × 32 tile, read whole: the ramp
/// `0.1 + j·2e-5` for j in 0..512, each value at a pixel and at its reflection through the tile's
/// centre, so the tile's plane is flat, exactly; the top 100 j are raised by 0.005, 200 pixels.
/// The median of the 1024 averages ranks 511 and 512, j = 255 and 256: 0.1 + 255.5·2e-5 =
/// 0.10511 (raising the top moves neither); the mean is that plus 200 · 0.005/1024 = 0.000977. The tail's largest deviation, 0.0101, sits inside 3σ ≈ 0.0114, so
/// nothing clips; `|mean − median|` is under 0.3σ, so the mode applies: 0.105115 − 1.5 · 0.000977 =
/// 0.103650. The inputs round to f32 by 3.7e-9 at most, the Pearson weights carry that four times
/// over, and the products and difference round once each near 0.26: 1e-7 holds it.
#[test]
fn skewed_tile_sky_sits_below_median() {
    let ramp = |j: usize| 0.1 + j as f32 * 2e-5 + if j >= 412 { 0.005 } else { 0.0 };
    let pixels = Buffer2::new(32, 32, (0..1024).map(|i| ramp(i.min(1023 - i))).collect());
    let sky = make_grid(&pixels, 32).stats[(0, 0)].sky;
    let median = 0.1 + 255.5 * 2e-5;
    let expected = median - 1.5 * (200.0 * 0.005 / 1024.0);
    assert!(
        (f64::from(sky) - expected).abs() < 1e-7,
        "Pearson-mode sky {expected}, got {sky}"
    );
}

/// Masked pixels never reach a tile's statistics. Tile (1,0) keeps 0.4 around a masked 16 × 16
/// block at 5.0, three quarters of it left, so it reads 0.4 exactly. Tile (0,0) keeps two
/// unmasked rows of 32 at 0.2 under 30 masked rows at 0.9 — read whole it would answer 0.9 — which
/// is under half of it: a bad tile, it takes the median of its good ring, 0.4, 0.6 and 0.8, so
/// 0.6 (review 18.5). A 2 × 2 grid is not median-filtered.
#[test]
fn masked_pixels_never_reach_the_statistics() {
    let mut pixels = Buffer2::new(
        64,
        64,
        (0..64 * 64)
            .map(|i| [[0.2f32, 0.4], [0.6, 0.8]][(i / 64) / 32][(i % 64) / 32])
            .collect(),
    );
    let mut mask = BitBuffer2::new_filled(Size2us::new(64, 64), false);
    for y in 2..32 {
        for x in 0..32 {
            pixels[(x, y)] = 0.9;
            mask.set_at(Vec2us::new(x, y), true);
        }
    }
    for y in 8..24 {
        for x in 40..56 {
            pixels[(x, y)] = 5.0;
            mask.set_at(Vec2us::new(x, y), true);
        }
    }
    let grid = make_grid_with_mask(&pixels, 32, &mask);
    for (tile, sky) in [((0, 0), 0.6), ((1, 0), 0.4), ((0, 1), 0.6), ((1, 1), 0.8)] {
        assert_eq!(grid.stats[tile].sky, sky, "tile {tile:?}");
    }
    assert_eq!(make_grid(&pixels, 32).stats[(0, 0)].sky, 0.9, "unmasked");
}

/// The tile statistics of 1..9 in one 3 × 3 tile: median 5, mean 5, so the Pearson mode is 5
/// too; the deviations 4, 3, 2, 1, 0, 1, 2, 3, 4 have the median 2, so σ is the MAD 2 rescaled,
/// and no value lies past 3σ ≈ 8.9.
#[test]
fn median_computation_correctness() {
    let pixels = Buffer2::new(3, 3, (1..=9).map(|x| x as f32).collect());
    let stats = make_grid(&pixels, 3).stats[(0, 0)];
    assert_eq!((stats.sky, stats.sigma), (5.0, mad_to_sigma(2.0f32)));
}

#[test]
fn mad_sigma_known_value() {
    // A 10×10 image where each row is [0,1,...,9] (pixel value = its x coordinate) keeps the
    // whole image in one 10×10 tile and gives 10 copies of each value.
    let width = 10;
    let height = 10;
    let data: Vec<f32> = (0..width * height).map(|i| (i % width) as f32).collect();
    let pixels = Buffer2::new(width, height, data);
    let grid = make_grid(&pixels, 10);
    let stats = grid.stats[(0, 0)];

    // Ten copies of 0..9: the median is (4 + 5)/2 = 4.5, equal to the mean, so the Pearson mode
    // gives sky = 2.5·4.5 − 1.5·4.5 = 4.5. The deviations |x − 4.5| are 0.5..4.5, twenty of
    // each, so ranks 49 and 50 of the hundred are both 2.5: σ = 1.4826·2.5. No value lies past
    // 3σ = 11.1 from the median, so the clip keeps all of them.
    assert_eq!(stats.sky, 4.5);
    assert_eq!(stats.sigma, mad_to_sigma(2.5f32));
}

/// Clipping removes bright outliers from a sky with spread, and then reads the sky alone. One
/// 32 × 32 tile: 1000 pixels at 100 + (i mod 10), a hundred of each of 100..109, and 24 at 10000,
/// each value at a pixel and at its reflection through the tile's centre, so the plane of any
/// band of them is flat, exactly.
///
/// The first pass ranks all 1024: ranks 511 and 512 are both 105, and of the deviations from it
/// (100 at 0, then 200 each at 1..4, 100 at 5, and the outliers) ranks 511 and 512 are 3, so
/// σ = 1.4826·3 and 3σ ≈ 13.3 keeps the sky and drops the outliers. The second pass over the 1000
/// left: the median averages 104 and 105 to 104.5, the deviations 0.5..4.5 two hundred each put
/// ranks 499 and 500 at 2.5, and 3σ ≈ 11.1 keeps every value, so the clip converges. The mean is
/// 104.5 as well, so the Pearson mode is 2.5·104.5 − 1.5·104.5 = 104.5, every step exact in f32.
///
/// With no clip passes the outliers stay: the median of all 1024 is 105, the mean 338.4 is far
/// past 0.3σ from it, and the sky falls back to that median.
#[test]
fn clipping_rejects_outliers_from_a_sky_with_spread() {
    let value = |i: usize| {
        if i < 500 {
            100.0 + (i % 10) as f32
        } else {
            10_000.0
        }
    };
    let pixels = Buffer2::new(32, 32, (0..1024).map(|i| value(i.min(1023 - i))).collect());

    let clipped = make_grid(&pixels, 32).stats[(0, 0)];
    assert_eq!((clipped.sky, clipped.sigma), (104.5, mad_to_sigma(2.5f32)));

    let unclipped = compute_grid(&pixels, None, 32, 0, true).stats[(0, 0)];
    assert_eq!(unclipped.sky, 105.0);
}

/// A tile's centre is the mean index of the pixels it holds, `(start + end − 1) / 2`: 32-wide tiles
/// sit at 15.5 + 32k, the third tile of 100, which took the remainder of 4, at (64 + 99) / 2, and a
/// tile clamped to a 20-px image at 9.5. Both axes from one rule.
#[test]
fn tile_centres_are_the_mean_pixel_index() {
    struct Case {
        size: Size2us,
        tile_size: usize,
        centers_x: &'static [f32],
        centers_y: &'static [f32],
    }
    let cases = [
        Case {
            size: Size2us::new(128, 64),
            tile_size: 32,
            centers_x: &[15.5, 47.5, 79.5, 111.5],
            centers_y: &[15.5, 47.5],
        },
        Case {
            size: Size2us::new(64, 100),
            tile_size: 32,
            centers_x: &[15.5, 47.5],
            centers_y: &[15.5, 47.5, 81.5],
        },
        Case {
            size: Size2us::new(32, 32),
            tile_size: 32,
            centers_x: &[15.5],
            centers_y: &[15.5],
        },
        Case {
            size: Size2us::new(20, 20),
            tile_size: 64,
            centers_x: &[9.5],
            centers_y: &[9.5],
        },
    ];
    for case in cases {
        let grid = make_grid(
            &Buffer2::new_filled(case.size.width, case.size.height, 0.5),
            case.tile_size,
        );
        assert_eq!(grid.centers_x, case.centers_x, "{:?}", case.size);
        assert_eq!(grid.centers_y, case.centers_y, "{:?}", case.size);
    }
}

/// The 3×3 median keeps a plane sky exactly at every tile, the edges and corners included, where a
/// window cut at the grid edge would pull each edge tile half a tile toward the interior. And it
/// drops spoiled tiles — one at a corner, one on an edge, one inside — to the sky around them:
/// none of the windows holds more than 4 spoiled values of 9, reflections counted.
///
/// The plane's 32×32 tiles are read whole and point-symmetric about their centres, so their
/// median and mean, and the Pearson mode from them, are the plane at the centre to a few f32
/// roundings of a value ≤ 1: 8ε bounds them.
#[test]
fn median_filter_keeps_a_plane_and_drops_spoiled_tiles() {
    let size = 160;
    let plane = |x: f32, y: f32| 0.1 + 1e-3 * x + 2e-3 * y;
    let pixels = Buffer2::new(
        size,
        size,
        (0..size * size)
            .map(|i| plane((i % size) as f32, (i / size) as f32))
            .collect(),
    );
    let grid = make_grid(&pixels, 32);
    let sigma = grid.stats[(2, 2)].sigma;
    for ty in 0..5 {
        for tx in 0..5 {
            let stats = grid.stats[(tx, ty)];
            let expected = plane(grid.centers_x[tx], grid.centers_y[ty]);
            assert!(
                (stats.sky - expected).abs() <= 8.0 * f32::EPSILON,
                "tile ({tx}, {ty}): sky {} vs {expected}",
                stats.sky
            );
            assert!(
                (stats.sigma - sigma).abs() <= 8.0 * f32::EPSILON,
                "tile ({tx}, {ty}): σ {} vs {sigma}",
                stats.sigma
            );
        }
    }

    let mut pixels = Buffer2::new_filled(size, size, 50.0f32);
    for (tx, ty) in [(0, 0), (2, 0), (2, 2)] {
        for y in 32 * ty..32 * (ty + 1) {
            for x in 32 * tx..32 * (tx + 1) {
                pixels[(x, y)] = 200.0;
            }
        }
    }
    let grid = make_grid(&pixels, 32);
    for (tile, stats) in grid.stats.pixels().iter().enumerate() {
        assert_eq!(stats.sky, 50.0, "tile {tile}");
    }
}

/// `find_lower_tile_y` is the last tile whose centre is at or before the position, and tile 0
/// before the first centre: 32-px tiles over 128 rows centre at 15.5, 47.5, 79.5, 111.5.
#[test]
fn find_lower_tile_y_is_the_last_centre_at_or_before() {
    let grid = make_grid(&Buffer2::new_filled(64, 128, 0.5), 32);
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
        assert_eq!(grid.find_lower_tile_y(pos), tile, "y = {pos}");
    }
    let single = make_grid(&Buffer2::new_filled(32, 32, 0.5), 32);
    for pos in [0.0, 15.5, 100.0] {
        assert_eq!(single.find_lower_tile_y(pos), 0, "one tile, y = {pos}");
    }
}

/// A tile masked whole takes its neighbour's sky: on the ramp `x/64`, tile (1, 0) reads the ramp at
/// its centre column, 47.5/64 (dyadic, exact), and the masked tile (0, 0) takes it. A mask over
/// every tile leaves no sky to take, and each tile reads all its pixels: 15.5/64 and 47.5/64.
#[test]
fn a_masked_tile_takes_its_neighbours_sky() {
    let pixels = Buffer2::new(
        64,
        32,
        (0..64 * 32).map(|i| (i % 64) as f32 / 64.0).collect(),
    );
    let mut mask = BitBuffer2::new_filled(Size2us::new(64, 32), false);
    for y in 0..32 {
        for x in 0..32 {
            mask.set_at(Vec2us::new(x, y), true);
        }
    }
    let grid = make_grid_with_mask(&pixels, 32, &mask);
    assert_eq!(grid.stats[(0, 0)].sky, 47.5 / 64.0);
    assert_eq!(grid.stats[(1, 0)].sky, 47.5 / 64.0);

    let everything = BitBuffer2::new_filled(Size2us::new(64, 32), true);
    let grid = make_grid_with_mask(&pixels, 32, &everything);
    assert_eq!(grid.stats[(0, 0)].sky, 15.5 / 64.0);
    assert_eq!(grid.stats[(1, 0)].sky, 47.5 / 64.0);

    // Four tiles of the ramp x/128, the first three masked: each searches outward ring by ring
    // past the bad ones, and all three take tile 3's 111.5/128.
    let wide = Buffer2::new(
        128,
        32,
        (0..128 * 32).map(|i| (i % 128) as f32 / 128.0).collect(),
    );
    let mut first_three = BitBuffer2::new_filled(Size2us::new(128, 32), false);
    for y in 0..32 {
        for x in 0..96 {
            first_three.set_at(Vec2us::new(x, y), true);
        }
    }
    let grid = make_grid_with_mask(&wide, 32, &first_three);
    for tile in 0..4 {
        assert_eq!(grid.stats[(tile, 0)].sky, 111.5 / 128.0, "tile {tile}");
    }
}

/// Under three tiles on an axis the 3×3 median does not run: a 2×2 grid of distinct flat tiles
/// keeps each tile's own sky, where a filter would have moved every one.
#[test]
fn a_grid_under_three_tiles_is_not_filtered() {
    let skies = [[0.1f32, 0.2], [0.3, 0.9]];
    let pixels = Buffer2::new(
        64,
        64,
        (0..64 * 64)
            .map(|i| skies[(i / 64) / 32][(i % 64) / 32])
            .collect(),
    );
    let grid = make_grid(&pixels, 32);
    for (ty, row) in skies.iter().enumerate() {
        for (tx, &sky) in row.iter().enumerate() {
            assert_eq!(grid.stats[(tx, ty)].sky, sky, "tile ({tx}, {ty})");
        }
    }
}

/// The y spline's second derivatives through the mesh: four tile rows of skies `k²·c` (c = 1/64)
/// at a uniform 32 px solve, as `solve_d2_quadratic_data` does on unit spacing, to `d″ = [0, 2.4,
/// 2.4, 0] · c/32²`, in every tile column; the flat σ plane has none. The 3×3 median keeps the
/// rows: each window, reflected at the ends, is centred on its own value.
#[test]
fn y_spline_derivatives_through_the_mesh() {
    const C: f32 = 1.0 / 64.0;
    let pixels = Buffer2::new(
        96,
        128,
        (0..96 * 128)
            .map(|i| {
                let k = (i / 96 / 32) as f32;
                k * k * C
            })
            .collect(),
    );
    let grid = make_grid(&pixels, 32);
    let interior = 2.4 * C / 1024.0;
    for tx in 0..3 {
        for (ty, expected) in [0.0, interior, interior, 0.0].into_iter().enumerate() {
            let tile = Vec2us::new(tx, ty);
            let d2 = grid.d2y(TileComponent::Sky, tile);
            assert!(
                (d2 - expected).abs() <= 4.0 * f32::EPSILON * interior,
                "sky d″ at ({tx}, {ty}): {d2} vs {expected}"
            );
            assert_eq!(
                grid.d2y(TileComponent::Sigma, tile),
                0.0,
                "σ d″ at ({tx}, {ty})"
            );
        }
    }
}
