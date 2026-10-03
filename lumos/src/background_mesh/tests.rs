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
fn new_uninit_clamps_tile_size_to_image() {
    // tile_size larger than the image must clamp to min(width, height) = 8 instead of
    // producing a 0-tile grid: 10.div_ceil(8) = 2 tiles in x, 8.div_ceil(8) = 1 in y.
    let grid = TileGrid::new_uninit(Size2us::new(10, 8), 64);
    assert_eq!(grid.stats.width(), 2);
    assert_eq!(grid.stats.height(), 1);
}

#[test]
#[should_panic(expected = "non-zero")]
fn new_uninit_zero_dimension_panics() {
    // A zero-size image is a logic error upstream (ImageDimensions asserts > 0); fail fast
    // with a clear message instead of a bare div_ceil divide-by-zero.
    TileGrid::new_uninit(Size2us::new(0, 100), 64);
}

#[test]
fn skewed_tile_sky_sits_below_median() {
    // One 32×32 tile: a symmetric ramp 0.1 + i·1e-5 (i = 0..1024) whose top 200 values get
    // +0.005 — a bright-ward tail that survives 3σ clipping (max deviation ≈ 0.0101 < 3σ ≈
    // 0.0114). Hand-computed: median ≈ 0.10512 (unchanged by shifting the top values),
    // mean = median + 200·0.005/1024 ≈ median + 0.00098, |mean−median| < 0.3σ → mode fires:
    //   sky = 2.5·0.10512 − 1.5·0.10610 ≈ 0.10365
    // — below the median-only estimate by ~1.5e-3.
    let n = 1024usize;
    let mut values: Vec<f32> = (0..n).map(|i| 0.1 + i as f32 * 1e-5).collect();
    for v in values.iter_mut().skip(n - 200) {
        *v += 0.005;
    }
    let pixels = Buffer2::new(32, 32, values);
    let grid = make_grid(&pixels, 32);
    let sky = grid.stats[(0, 0)].sky;
    assert!(
        (sky - 0.10365).abs() < 5e-4,
        "Pearson-mode sky ≈ 0.10365, got {sky}"
    );
    assert!(
        sky < 0.1045,
        "sky must sit below the median-only estimate (≈0.10512), got {sky}"
    );
}

#[test]
fn tile_grid_dimensions() {
    let pixels = Buffer2::new_filled(128, 64, 0.5);
    let grid = make_grid(&pixels, 32);

    assert_eq!(grid.stats.width(), 4);
    assert_eq!(grid.stats.height(), 2);
}

#[test]
fn tile_grid_dimensions_non_divisible() {
    let pixels = Buffer2::new_filled(100, 70, 0.5);
    let grid = make_grid(&pixels, 32);

    assert_eq!(grid.stats.width(), 4);
    assert_eq!(grid.stats.height(), 3);
}

#[test]
fn tile_grid_uniform_image() {
    let pixels = Buffer2::new_filled(64, 64, 0.3);
    let grid = make_grid(&pixels, 32);

    for ty in 0..grid.stats.height() {
        for tx in 0..grid.stats.width() {
            let stats = grid.stats[(tx, ty)];
            assert!((stats.sky - 0.3).abs() < 0.01);
            assert!(stats.sigma < 0.01);
        }
    }
}

#[test]
fn tile_grid_with_mask_excludes_masked() {
    let width = 64;
    let height = 64;
    let mut pixels = Buffer2::new_filled(width, height, 0.2);

    for y in 0..32 {
        for x in 0..32 {
            pixels[(x, y)] = 0.8;
        }
    }

    let mut mask = BitBuffer2::new_filled(Size2us::new(width, height), false);
    for y in 0..32 {
        for x in 0..32 {
            mask.set_at(Vec2us::new(x, y), true);
        }
    }

    let grid = make_grid_with_mask(&pixels, 32, &mask);

    let stats_11 = grid.stats[(1, 1)];
    assert!((stats_11.sky - 0.2).abs() < 0.05);
}

#[test]
fn tile_uses_few_unmasked_pixels_over_all_pixels() {
    // Tile (0,0) has 95% masked "star" pixels at 0.9, 5% unmasked background at 0.2.
    // The unmasked pixels should be used for background estimation (median ≈ 0.2),
    // NOT falling back to all pixels which would give a biased median toward 0.9.
    let width = 64;
    let height = 64;

    // Start with all pixels at "star" value
    let mut pixels = Buffer2::new_filled(width, height, 0.9f32);

    // Set ~5% of the top-left tile (32×32 = 1024 pixels) to background value
    // 5% of 1024 = ~51 pixels. Use a stripe: first 2 rows unmasked.
    // 2 rows × 32 cols = 64 pixels of background
    let mut mask = BitBuffer2::new_filled(Size2us::new(width, height), false);
    for y in 0..32 {
        for x in 0..32 {
            if y < 2 {
                // Background pixels: unmasked, value 0.2
                pixels[(x, y)] = 0.2;
                // mask stays false (unmasked)
            } else {
                // Star pixels: masked, value 0.9
                mask.set_at(Vec2us::new(x, y), true);
            }
        }
    }

    let grid = make_grid_with_mask(&pixels, 32, &mask);

    let stats = grid.stats[(0, 0)];
    // With the fix: uses the 64 unmasked background pixels → median ≈ 0.2
    // Without the fix: falls back to all 1024 pixels → median biased toward 0.9
    assert!(
        (stats.sky - 0.2).abs() < 0.05,
        "Tile (0,0) median should be ~0.2 (background), got {}",
        stats.sky
    );
}

#[test]
fn tile_stats_with_gradient() {
    let width = 64;
    let height = 64;
    let data: Vec<f32> = (0..height)
        .flat_map(|y| (0..width).map(move |x| (x + y) as f32 / 128.0))
        .collect();

    let pixels = Buffer2::new(width, height, data);
    let grid = make_grid(&pixels, 32);

    let tl = grid.stats[(0, 0)];
    let br = grid.stats[(1, 1)];
    assert!(br.sky > tl.sky);
}

#[test]
fn large_tile_size() {
    // A tile size beyond the image clamps to min(w, h) = 50 → 100.div_ceil(50) = 2 x 1 tiles.
    let pixels = Buffer2::new_filled(100, 50, 0.3);
    let grid = make_grid(&pixels, 200);

    assert_eq!(grid.stats.width(), 2);
    assert_eq!(grid.stats.height(), 1);

    for tx in 0..grid.stats.width() {
        let stats = grid.stats[(tx, 0)];
        assert!((stats.sky - 0.3).abs() < 0.01);
    }
}

#[test]
fn tile_grid_very_wide_image() {
    // tile_size clamps to min(w, h) = 10 → 100 x 1 tiles of 10x10.
    let pixels = Buffer2::new_filled(1000, 10, 0.5);
    let grid = make_grid(&pixels, 64);

    assert_eq!(grid.stats.width(), 100);
    assert_eq!(grid.stats.height(), 1);

    for tx in 0..grid.stats.width() {
        let stats = grid.stats[(tx, 0)];
        assert!((stats.sky - 0.5).abs() < 0.01);
    }
}

#[test]
fn tile_grid_very_tall_image() {
    // tile_size clamps to min(w, h) = 10 → 1 x 100 tiles of 10x10.
    let pixels = Buffer2::new_filled(10, 1000, 0.5);
    let grid = make_grid(&pixels, 64);

    assert_eq!(grid.stats.width(), 1);
    assert_eq!(grid.stats.height(), 100);

    for ty in 0..grid.stats.height() {
        let stats = grid.stats[(0, ty)];
        assert!((stats.sky - 0.5).abs() < 0.01);
    }
}

#[test]
fn tile_with_outliers_sigma_clipped() {
    let width = 64;
    let height = 64;
    let mut pixels = Buffer2::new_filled(width, height, 0.5);

    // Add some outliers
    for val in pixels.iter_mut().take(10) {
        *val = 10.0; // Bright outliers
    }

    let grid = make_grid(&pixels, 64);

    let stats = grid.stats[(0, 0)];
    // Median should be close to 0.5 despite outliers
    assert!((stats.sky - 0.5).abs() < 0.1);
}

#[test]
fn tile_stats_sigma_nonzero_for_varied_data() {
    let width = 64;
    let height = 64;
    // Create data with variation
    let data: Vec<f32> = (0..width * height)
        .map(|i| 0.5 + (i % 10) as f32 * 0.01)
        .collect();

    let pixels = Buffer2::new(width, height, data);
    let grid = make_grid(&pixels, 64);

    let stats = grid.stats[(0, 0)];
    assert!(stats.sigma > 0.0);
}

#[test]
fn negative_pixel_values() {
    let width = 64;
    let height = 64;
    let data = vec![-0.5; width * height];

    let pixels = Buffer2::new(width, height, data);
    let grid = make_grid(&pixels, 32);

    let stats = grid.stats[(0, 0)];
    assert!((stats.sky - (-0.5)).abs() < 0.01);
}

#[test]
fn median_computation_correctness() {
    // Create image where we know exact median
    // Tile with values 1,2,3,4,5,6,7,8,9 should have median=5
    let width = 3;
    let height = 3;
    let data: Vec<f32> = (1..=9).map(|x| x as f32).collect();

    let pixels = Buffer2::new(width, height, data);
    let grid = make_grid(&pixels, 3);

    let stats = grid.stats[(0, 0)];
    assert!(
        (stats.sky - 5.0).abs() < 0.1,
        "Median of 1-9 should be 5, got {}",
        stats.sky
    );
}

#[test]
fn sigma_computation_correctness() {
    // For uniform data, sigma should be 0
    let pixels = Buffer2::new_filled(64, 64, 100.0);
    let grid = make_grid(&pixels, 64);

    let stats = grid.stats[(0, 0)];
    assert!(
        stats.sigma < 0.001,
        "Uniform data should have sigma ~0, got {}",
        stats.sigma
    );
}

#[test]
fn mad_sigma_known_value() {
    // MAD-based sigma for a known distribution. A 10x10 image where each row is
    // [0,1,...,9] (pixel value = its x coordinate) keeps the whole image in one 10x10
    // tile and gives 10 copies of each value, so the order statistics match the plain
    // [0..9] case:
    // - Approximate median (used for performance) = 5 (upper-middle for even length)
    // - Deviations from median: 10 copies each of [5,4,3,2,1,0,1,2,3,4]
    // - MAD = approximate median of deviations = 3
    // - sigma = MAD * 1.4826 ≈ 4.4
    let width = 10;
    let height = 10;
    let data: Vec<f32> = (0..width * height).map(|i| (i % width) as f32).collect();

    let pixels = Buffer2::new(width, height, data);

    // One tile covering all pixels
    let grid = make_grid(&pixels, 10);
    let stats = grid.stats[(0, 0)];

    // Ten copies of 0..9: the median is (4 + 5)/2 = 4.5, equal to the mean, so the Pearson mode
    // gives sky = 2.5·4.5 − 1.5·4.5 = 4.5. The deviations |x − 4.5| are 0.5..4.5, twenty of
    // each, so ranks 49 and 50 of the hundred are both 2.5: σ = 1.4826·2.5. No value lies past
    // 3σ = 11.1 from the median, so the clip keeps all of them.
    assert_eq!(stats.sky, 4.5);
    assert_eq!(stats.sigma, mad_to_sigma(2.5f32));
}

#[test]
fn sigma_sigma_clipping_rejects_outliers() {
    // Background of 100 with a few extreme outliers
    // 3-sigma clipping should reject values > median + 3*sigma
    let width = 100;
    let height = 100;
    let mut pixels = Buffer2::new_filled(width, height, 100.0);

    // Add 1% extreme outliers (100 pixels with value 10000)
    for i in 0..100 {
        pixels[i * 100] = 10000.0;
    }

    let grid = make_grid(&pixels, 100);

    let stats = grid.stats[(0, 0)];

    // After sigma clipping, median should still be ~100
    assert!(
        (stats.sky - 100.0).abs() < 5.0,
        "Median should be ~100 after clipping outliers, got {}",
        stats.sky
    );
}

#[test]
fn background_gradient_preserved() {
    // Linear gradient from 0 to 100 across image
    // Tile statistics should reflect local background level
    let width = 256;
    let height = 64;
    let data: Vec<f32> = (0..height)
        .flat_map(|_| (0..width).map(|x| x as f32 / width as f32 * 100.0))
        .collect();

    let pixels = Buffer2::new(width, height, data);
    let grid = make_grid(&pixels, 64);

    // Left tiles should have lower median than right tiles
    let left = grid.stats[(0, 0)];
    let right = grid.stats[(3, 0)];

    assert!(
        right.sky > left.sky + 30.0,
        "Right tile median {} should be > left {} + 30",
        right.sky,
        left.sky
    );
    assert!(
        left.sky < 30.0,
        "Left tile median {} should be < 30",
        left.sky
    );
    assert!(
        right.sky > 70.0,
        "Right tile median {} should be > 70",
        right.sky
    );
}

#[test]
fn sparse_stars_rejected() {
    // Simulate astronomical image: mostly background (100) with sparse bright stars
    let width = 128;
    let height = 128;
    let mut pixels = Buffer2::new_filled(width, height, 100.0);

    // Add 20 "stars" with brightness 500-1000 (random positions)
    let star_positions = [
        (10, 10),
        (50, 20),
        (100, 30),
        (30, 60),
        (80, 70),
        (120, 80),
        (15, 100),
        (60, 110),
        (90, 120),
        (110, 115),
        (25, 25),
        (75, 45),
        (45, 75),
        (95, 95),
        (5, 55),
        (55, 5),
        (105, 55),
        (55, 105),
        (35, 35),
        (85, 85),
    ];

    for (x, y) in star_positions {
        // Star with some spread
        for dy in -1i32..=1 {
            for dx in -1i32..=1 {
                let nx = (x + dx).clamp(0, 127) as usize;
                let ny = (y + dy).clamp(0, 127) as usize;
                pixels[(nx, ny)] = 500.0 + (dx.abs() + dy.abs()) as f32 * -100.0;
            }
        }
    }

    let grid = make_grid(&pixels, 64);

    // All tiles should have median close to background (100)
    for ty in 0..grid.stats.height() {
        for tx in 0..grid.stats.width() {
            let stats = grid.stats[(tx, ty)];
            assert!(
                (stats.sky - 100.0).abs() < 20.0,
                "Tile ({},{}) median {} should be ~100 (background)",
                tx,
                ty,
                stats.sky
            );
        }
    }
}

#[test]
fn mask_excludes_sources_correctly() {
    // Background 50, sources at 200
    let width = 64;
    let height = 64;
    let mut pixels = Buffer2::new_filled(width, height, 50.0);

    // Add bright source in top-left quadrant
    for y in 0..32 {
        for x in 0..32 {
            pixels[(x, y)] = 200.0;
        }
    }

    // Mask the bright source
    let mut mask = BitBuffer2::new_filled(Size2us::new(width, height), false);
    for y in 0..32 {
        for x in 0..32 {
            mask.set_at(Vec2us::new(x, y), true);
        }
    }

    let grid = make_grid_with_mask(&pixels, 32, &mask);

    // Top-left tile (0,0): all pixels masked → falls back to all pixels (200.0)
    // Bottom-right tile (1,1): no masked pixels → uses background value
    let br = grid.stats[(1, 1)];
    assert!(
        (br.sky - 50.0).abs() < 5.0,
        "Unmasked tile median {} should be ~50",
        br.sky
    );
}

#[test]
fn photutils_sextractor_comparison() {
    // Test case similar to photutils/SExtractor documentation examples
    // Background level 1000 with noise sigma ~10
    let width = 256;
    let height = 256;

    // Generate pseudo-random noise using deterministic pattern
    let data: Vec<f32> = (0..width * height)
        .map(|i| {
            let noise = ((i * 7919 + 104_729) % 1000) as f32 / 100.0 - 5.0; // -5 to +5
            1000.0 + noise * 2.0 // background 1000, noise ~10
        })
        .collect();

    let pixels = Buffer2::new(width, height, data);
    let grid = make_grid(&pixels, 64);

    // Check all tiles have reasonable background estimate
    for ty in 0..grid.stats.height() {
        for tx in 0..grid.stats.width() {
            let stats = grid.stats[(tx, ty)];
            // Background should be ~1000 ± 5
            assert!(
                (stats.sky - 1000.0).abs() < 10.0,
                "Tile ({},{}) median {} should be ~1000",
                tx,
                ty,
                stats.sky
            );
            // Sigma should be reasonable (not zero, not huge)
            assert!(
                stats.sigma > 1.0 && stats.sigma < 30.0,
                "Tile ({},{}) sigma {} should be reasonable",
                tx,
                ty,
                stats.sigma
            );
        }
    }
}

/// A tile's centre is the mean index of the pixels it holds, `(start + end − 1) / 2`: 32-wide tiles
/// sit at 15.5 + 32k, the 4-wide remainder of 100 at (96 + 99) / 2, and a tile clamped to a 20-px
/// image at 9.5. Both axes from one rule.
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
            centers_y: &[15.5, 47.5, 79.5, 97.5],
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

/// A tile masked whole reads every pixel instead: on the ramp `x/64` its sky is the ramp at the
/// tile's centre column, 15.5/64 (dyadic, exact), where an unmasked neighbour reads its own.
#[test]
fn a_wholly_masked_tile_reads_all_its_pixels() {
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
    assert_eq!(grid.stats[(0, 0)].sky, 15.5 / 64.0);
    assert_eq!(grid.stats[(1, 0)].sky, 47.5 / 64.0);
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
