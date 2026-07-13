use super::*;

/// Number of sigma-clipping iterations for tests.
const TEST_SIGMA_CLIP_ITERATIONS: usize = 2;

fn create_uniform_image(width: usize, height: usize, value: f32) -> Buffer2<f32> {
    Buffer2::new(width, height, vec![value; width * height])
}

/// Create a TileGrid with default test parameters (no mask, default sigma clip iterations)
fn make_grid(pixels: &Buffer2<f32>, tile_size: usize) -> TileGrid {
    let mut grid = TileGrid::new_uninit(pixels.width(), pixels.height(), tile_size);
    grid.compute(pixels, None, TEST_SIGMA_CLIP_ITERATIONS, true);
    grid
}

/// Create a TileGrid with mask
fn make_grid_with_mask(pixels: &Buffer2<f32>, tile_size: usize, mask: &BitBuffer2) -> TileGrid {
    let mut grid = TileGrid::new_uninit(pixels.width(), pixels.height(), tile_size);
    grid.compute(pixels, Some(mask), TEST_SIGMA_CLIP_ITERATIONS, true);
    grid
}

#[test]
fn new_uninit_clamps_tile_size_to_image() {
    // tile_size larger than the image must clamp to min(width, height) = 8 instead of
    // producing a 0-tile grid: 10.div_ceil(8) = 2 tiles in x, 8.div_ceil(8) = 1 in y.
    let grid = TileGrid::new_uninit(10, 8, 64);
    assert_eq!(grid.tiles_x(), 2);
    assert_eq!(grid.tiles_y(), 1);
}

#[test]
#[should_panic(expected = "non-zero")]
fn new_uninit_zero_dimension_panics() {
    // A zero-size image is a logic error upstream (ImageDimensions asserts > 0); fail fast
    // with a clear message instead of a bare div_ceil divide-by-zero.
    TileGrid::new_uninit(0, 100, 64);
}

// --- SExtractor sky estimator ---

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
    let sky = grid.get(0, 0).sky;
    assert!(
        (sky - 0.10365).abs() < 5e-4,
        "Pearson-mode sky ≈ 0.10365, got {sky}"
    );
    assert!(
        sky < 0.1045,
        "sky must sit below the median-only estimate (≈0.10512), got {sky}"
    );
}

// --- Construction ---

#[test]
fn test_tile_grid_dimensions() {
    let pixels = create_uniform_image(128, 64, 0.5);
    let grid = make_grid(&pixels, 32);

    assert_eq!(grid.tiles_x(), 4);
    assert_eq!(grid.tiles_y(), 2);
}

#[test]
fn test_tile_grid_dimensions_non_divisible() {
    let pixels = create_uniform_image(100, 70, 0.5);
    let grid = make_grid(&pixels, 32);

    assert_eq!(grid.tiles_x(), 4);
    assert_eq!(grid.tiles_y(), 3);
}

#[test]
fn test_tile_grid_uniform_image() {
    let pixels = create_uniform_image(64, 64, 0.3);
    let grid = make_grid(&pixels, 32);

    for ty in 0..grid.tiles_y() {
        for tx in 0..grid.tiles_x() {
            let stats = grid.get(tx, ty);
            assert!((stats.sky - 0.3).abs() < 0.01);
            assert!(stats.sigma < 0.01);
        }
    }
}

// --- Center computation ---

#[test]
fn test_center_x_full_tiles() {
    let pixels = create_uniform_image(128, 64, 0.5);
    let grid = make_grid(&pixels, 32);

    assert!((grid.centers_x[0] - 16.0).abs() < 0.01);
    assert!((grid.centers_x[1] - 48.0).abs() < 0.01);
    assert!((grid.centers_x[2] - 80.0).abs() < 0.01);
    assert!((grid.centers_x[3] - 112.0).abs() < 0.01);
}

#[test]
fn test_center_x_partial_tile() {
    let pixels = create_uniform_image(100, 64, 0.5);
    let grid = make_grid(&pixels, 32);

    assert!((grid.centers_x[3] - 98.0).abs() < 0.01);
}

#[test]
fn test_center_y_full_tiles() {
    let pixels = create_uniform_image(64, 128, 0.5);
    let grid = make_grid(&pixels, 32);

    assert!((grid.center_y(0) - 16.0).abs() < 0.01);
    assert!((grid.center_y(1) - 48.0).abs() < 0.01);
    assert!((grid.center_y(2) - 80.0).abs() < 0.01);
    assert!((grid.center_y(3) - 112.0).abs() < 0.01);
}

#[test]
fn test_center_y_partial_tile() {
    let pixels = create_uniform_image(64, 100, 0.5);
    let grid = make_grid(&pixels, 32);

    assert!((grid.center_y(3) - 98.0).abs() < 0.01);
}

// --- find_lower_tile_y ---

#[test]
fn test_find_lower_tile_y_exact_center() {
    let pixels = create_uniform_image(64, 128, 0.5);
    let grid = make_grid(&pixels, 32);

    assert_eq!(grid.find_lower_tile_y(16.0), 0);
    assert_eq!(grid.find_lower_tile_y(48.0), 1);
    assert_eq!(grid.find_lower_tile_y(80.0), 2);
    assert_eq!(grid.find_lower_tile_y(112.0), 3);
}

#[test]
fn test_find_lower_tile_y_between_centers() {
    let pixels = create_uniform_image(64, 128, 0.5);
    let grid = make_grid(&pixels, 32);

    assert_eq!(grid.find_lower_tile_y(30.0), 0);
    assert_eq!(grid.find_lower_tile_y(60.0), 1);
    assert_eq!(grid.find_lower_tile_y(100.0), 2);
}

#[test]
fn test_find_lower_tile_y_before_first_center() {
    let pixels = create_uniform_image(64, 128, 0.5);
    let grid = make_grid(&pixels, 32);

    assert_eq!(grid.find_lower_tile_y(0.0), 0);
    assert_eq!(grid.find_lower_tile_y(10.0), 0);
}

#[test]
fn test_find_lower_tile_y_after_last_center() {
    let pixels = create_uniform_image(64, 128, 0.5);
    let grid = make_grid(&pixels, 32);

    assert_eq!(grid.find_lower_tile_y(120.0), 3);
    assert_eq!(grid.find_lower_tile_y(1000.0), 3);
}

#[test]
fn test_find_lower_tile_y_single_tile() {
    let pixels = create_uniform_image(32, 32, 0.5);
    let grid = make_grid(&pixels, 32);

    assert_eq!(grid.tiles_y(), 1);
    assert_eq!(grid.find_lower_tile_y(0.0), 0);
    assert_eq!(grid.find_lower_tile_y(16.0), 0);
    assert_eq!(grid.find_lower_tile_y(100.0), 0);
}

// --- Mask handling ---

#[test]
fn test_tile_grid_with_mask_excludes_masked() {
    let width = 64;
    let height = 64;
    let mut data = vec![0.2; width * height];

    for y in 0..32 {
        for x in 0..32 {
            data[y * width + x] = 0.8;
        }
    }

    let pixels = Buffer2::new(width, height, data);

    let mut mask = BitBuffer2::new_filled(width, height, false);
    for y in 0..32 {
        for x in 0..32 {
            mask.set_xy(x, y, true);
        }
    }

    let grid = make_grid_with_mask(&pixels, 32, &mask);

    let stats_11 = grid.get(1, 1);
    assert!((stats_11.sky - 0.2).abs() < 0.05);
}

#[test]
fn test_tile_uses_few_unmasked_pixels_over_all_pixels() {
    // Tile (0,0) has 95% masked "star" pixels at 0.9, 5% unmasked background at 0.2.
    // The unmasked pixels should be used for background estimation (median ≈ 0.2),
    // NOT falling back to all pixels which would give a biased median toward 0.9.
    let width = 64;
    let height = 64;

    // Start with all pixels at "star" value
    let mut data = vec![0.9f32; width * height];

    // Set ~5% of the top-left tile (32×32 = 1024 pixels) to background value
    // 5% of 1024 = ~51 pixels. Use a stripe: first 2 rows unmasked.
    // 2 rows × 32 cols = 64 pixels of background
    let mut mask = BitBuffer2::new_filled(width, height, false);
    for y in 0..32 {
        for x in 0..32 {
            if y < 2 {
                // Background pixels: unmasked, value 0.2
                data[y * width + x] = 0.2;
                // mask stays false (unmasked)
            } else {
                // Star pixels: masked, value 0.9
                mask.set_xy(x, y, true);
            }
        }
    }

    let pixels = Buffer2::new(width, height, data);
    let grid = make_grid_with_mask(&pixels, 32, &mask);

    let stats = grid.get(0, 0);
    // With the fix: uses the 64 unmasked background pixels → median ≈ 0.2
    // Without the fix: falls back to all 1024 pixels → median biased toward 0.9
    assert!(
        (stats.sky - 0.2).abs() < 0.05,
        "Tile (0,0) median should be ~0.2 (background), got {}",
        stats.sky
    );
}

#[test]
fn test_all_pixels_masked_fallback() {
    let width = 64;
    let height = 64;
    let pixels = create_uniform_image(width, height, 0.4);
    let mask = BitBuffer2::new_filled(width, height, true);

    let grid = make_grid_with_mask(&pixels, 32, &mask);

    let stats = grid.get(0, 0);
    assert!((stats.sky - 0.4).abs() < 0.05);
}

#[test]
fn test_no_mask_same_as_none() {
    let pixels = create_uniform_image(64, 64, 0.5);

    let grid_none = make_grid(&pixels, 32);

    let mut grid_empty = TileGrid::new_uninit(64, 64, 32);
    grid_empty.compute(&pixels, None, TEST_SIGMA_CLIP_ITERATIONS, true);

    for ty in 0..grid_none.tiles_y() {
        for tx in 0..grid_none.tiles_x() {
            let s1 = grid_none.get(tx, ty);
            let s2 = grid_empty.get(tx, ty);
            assert!((s1.sky - s2.sky).abs() < 0.001);
        }
    }
}

// --- Median filter ---

#[test]
fn test_median_filter_uniform_unchanged() {
    let pixels = create_uniform_image(128, 128, 0.4);
    let grid = make_grid(&pixels, 32);

    for ty in 0..grid.tiles_y() {
        for tx in 0..grid.tiles_x() {
            let stats = grid.get(tx, ty);
            assert!((stats.sky - 0.4).abs() < 0.01);
        }
    }
}

#[test]
fn test_median_filter_rejects_outlier_tile() {
    let width = 128;
    let height = 128;
    let mut data = vec![0.3; width * height];

    for y in 32..64 {
        for x in 32..64 {
            data[y * width + x] = 0.9;
        }
    }

    let pixels = Buffer2::new(width, height, data);
    let grid = make_grid(&pixels, 32);

    let center_stats = grid.get(1, 1);
    assert!((center_stats.sky - 0.3).abs() < 0.1);
}

#[test]
fn test_median_filter_skipped_for_small_grid() {
    let pixels = create_uniform_image(64, 64, 0.5);
    let grid = make_grid(&pixels, 32);

    assert_eq!(grid.tiles_x(), 2);
    assert_eq!(grid.tiles_y(), 2);

    let stats = grid.get(0, 0);
    assert!((stats.sky - 0.5).abs() < 0.01);
}

// --- Edge cases ---

#[test]
fn test_single_tile_image() {
    let pixels = create_uniform_image(32, 32, 0.6);
    let grid = make_grid(&pixels, 32);

    assert_eq!(grid.tiles_x(), 1);
    assert_eq!(grid.tiles_y(), 1);

    let stats = grid.get(0, 0);
    assert!((stats.sky - 0.6).abs() < 0.01);
    assert!((grid.centers_x[0] - 16.0).abs() < 0.01);
    assert!((grid.center_y(0) - 16.0).abs() < 0.01);
}

#[test]
fn test_tile_stats_with_gradient() {
    let width = 64;
    let height = 64;
    let data: Vec<f32> = (0..height)
        .flat_map(|y| (0..width).map(move |x| (x + y) as f32 / 128.0))
        .collect();

    let pixels = Buffer2::new(width, height, data);
    let grid = make_grid(&pixels, 32);

    let tl = grid.get(0, 0);
    let br = grid.get(1, 1);
    assert!(br.sky > tl.sky);
}

#[test]
fn test_debug_impl() {
    let pixels = create_uniform_image(64, 64, 0.5);
    let grid = make_grid(&pixels, 32);

    let debug_str = format!("{:?}", grid);
    assert!(debug_str.contains("TileGrid"));
}

#[test]
fn test_image_smaller_than_tile() {
    let pixels = create_uniform_image(20, 20, 0.7);
    let grid = make_grid(&pixels, 64);

    assert_eq!(grid.tiles_x(), 1);
    assert_eq!(grid.tiles_y(), 1);

    let stats = grid.get(0, 0);
    assert!((stats.sky - 0.7).abs() < 0.01);
    assert!((grid.centers_x[0] - 10.0).abs() < 0.01);
    assert!((grid.center_y(0) - 10.0).abs() < 0.01);
}

#[test]
fn test_large_tile_size() {
    // A tile size beyond the image clamps to min(w, h) = 50 → 100.div_ceil(50) = 2 x 1 tiles.
    let pixels = create_uniform_image(100, 50, 0.3);
    let grid = make_grid(&pixels, 200);

    assert_eq!(grid.tiles_x(), 2);
    assert_eq!(grid.tiles_y(), 1);

    for tx in 0..grid.tiles_x() {
        let stats = grid.get(tx, 0);
        assert!((stats.sky - 0.3).abs() < 0.01);
    }
}

#[test]
fn test_tile_grid_very_wide_image() {
    // tile_size clamps to min(w, h) = 10 → 100 x 1 tiles of 10x10.
    let pixels = create_uniform_image(1000, 10, 0.5);
    let grid = make_grid(&pixels, 64);

    assert_eq!(grid.tiles_x(), 100);
    assert_eq!(grid.tiles_y(), 1);

    for tx in 0..grid.tiles_x() {
        let stats = grid.get(tx, 0);
        assert!((stats.sky - 0.5).abs() < 0.01);
    }
}

#[test]
fn test_tile_grid_very_tall_image() {
    // tile_size clamps to min(w, h) = 10 → 1 x 100 tiles of 10x10.
    let pixels = create_uniform_image(10, 1000, 0.5);
    let grid = make_grid(&pixels, 64);

    assert_eq!(grid.tiles_x(), 1);
    assert_eq!(grid.tiles_y(), 100);

    for ty in 0..grid.tiles_y() {
        let stats = grid.get(0, ty);
        assert!((stats.sky - 0.5).abs() < 0.01);
    }
}

// --- Helper function tests ---

#[test]
fn test_tile_with_outliers_sigma_clipped() {
    let width = 64;
    let height = 64;
    let mut data = vec![0.5; width * height];

    // Add some outliers
    for val in data.iter_mut().take(10) {
        *val = 10.0; // Bright outliers
    }

    let pixels = Buffer2::new(width, height, data);
    let grid = make_grid(&pixels, 64);

    let stats = grid.get(0, 0);
    // Median should be close to 0.5 despite outliers
    assert!((stats.sky - 0.5).abs() < 0.1);
}

#[test]
fn test_tile_stats_sigma_nonzero_for_varied_data() {
    let width = 64;
    let height = 64;
    // Create data with variation
    let data: Vec<f32> = (0..width * height)
        .map(|i| 0.5 + (i % 10) as f32 * 0.01)
        .collect();

    let pixels = Buffer2::new(width, height, data);
    let grid = make_grid(&pixels, 64);

    let stats = grid.get(0, 0);
    assert!(stats.sigma > 0.0);
}

#[test]
fn test_median_filter_corner_tiles() {
    // Test that corner tiles (with fewer neighbors) are handled correctly
    let pixels = create_uniform_image(128, 128, 0.5);
    let grid = make_grid(&pixels, 32);

    // Corner tiles should still have valid stats
    let corners = [(0, 0), (3, 0), (0, 3), (3, 3)];
    for (tx, ty) in corners {
        let stats = grid.get(tx, ty);
        assert!((stats.sky - 0.5).abs() < 0.01);
    }
}

#[test]
fn test_negative_pixel_values() {
    let width = 64;
    let height = 64;
    let data = vec![-0.5; width * height];

    let pixels = Buffer2::new(width, height, data);
    let grid = make_grid(&pixels, 32);

    let stats = grid.get(0, 0);
    assert!((stats.sky - (-0.5)).abs() < 0.01);
}

#[test]
fn test_find_lower_tile_y_negative_pos() {
    let pixels = create_uniform_image(64, 128, 0.5);
    let grid = make_grid(&pixels, 32);

    // Negative position should return 0
    assert_eq!(grid.find_lower_tile_y(-10.0), 0);
}

// --- Algorithm correctness tests ---
// These verify the statistical algorithms produce mathematically correct results

#[test]
fn test_median_computation_correctness() {
    // Create image where we know exact median
    // Tile with values 1,2,3,4,5,6,7,8,9 should have median=5
    let width = 3;
    let height = 3;
    let data: Vec<f32> = (1..=9).map(|x| x as f32).collect();

    let pixels = Buffer2::new(width, height, data);
    let grid = make_grid(&pixels, 3);

    let stats = grid.get(0, 0);
    assert!(
        (stats.sky - 5.0).abs() < 0.1,
        "Median of 1-9 should be 5, got {}",
        stats.sky
    );
}

#[test]
fn test_sigma_computation_correctness() {
    // For uniform data, sigma should be 0
    let pixels = create_uniform_image(64, 64, 100.0);
    let grid = make_grid(&pixels, 64);

    let stats = grid.get(0, 0);
    assert!(
        stats.sigma < 0.001,
        "Uniform data should have sigma ~0, got {}",
        stats.sigma
    );
}

#[test]
fn test_mad_sigma_known_value() {
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
    let stats = grid.get(0, 0);

    // Approximate median for even-length array returns the upper-middle element (5), the
    // mean is 4.5, and |mean − median| = 0.5 < 0.3σ ≈ 1.33, so the Pearson mode fires:
    // sky = 2.5·5 − 1.5·4.5 = 5.75. (The 0.5 "skew" is the fast-median convention on a
    // 10-sample fixture; on real ≥1000-sample tiles the offset is negligible.)
    assert!(
        (stats.sky - 5.75).abs() < 0.1,
        "Pearson-mode sky should be 5.75, got {}",
        stats.sky
    );
    // Sigma should be ~4.4 (MAD=3 * 1.4826)
    assert!(
        (stats.sigma - 4.4).abs() < 0.5,
        "Sigma should be ~4.4, got {}",
        stats.sigma
    );
}

#[test]
fn test_3sigma_clipping_rejects_outliers() {
    // Background of 100 with a few extreme outliers
    // 3-sigma clipping should reject values > median + 3*sigma
    let width = 100;
    let height = 100;
    let mut data = vec![100.0; width * height];

    // Add 1% extreme outliers (100 pixels with value 10000)
    for i in 0..100 {
        data[i * 100] = 10000.0;
    }

    let pixels = Buffer2::new(width, height, data);
    let grid = make_grid(&pixels, 100);

    let stats = grid.get(0, 0);

    // After sigma clipping, median should still be ~100
    assert!(
        (stats.sky - 100.0).abs() < 5.0,
        "Median should be ~100 after clipping outliers, got {}",
        stats.sky
    );
}

#[test]
fn test_median_filter_3x3_correctness() {
    // Create 5x5 grid of tiles where center tile has outlier value
    // After 3x3 median filter, center should match neighbors
    let width = 160; // 5 tiles of 32 pixels
    let height = 160;
    let mut data = vec![50.0; width * height];

    // Make center tile (tile 2,2) have value 200
    for y in 64..96 {
        for x in 64..96 {
            data[y * width + x] = 200.0;
        }
    }

    let pixels = Buffer2::new(width, height, data);
    let grid = make_grid(&pixels, 32);

    // Center tile should be filtered to ~50 (median of 8x50 + 1x200 = 50)
    let center = grid.get(2, 2);
    assert!(
        (center.sky - 50.0).abs() < 10.0,
        "Center tile should be ~50 after median filter, got {}",
        center.sky
    );
}

#[test]
fn test_background_gradient_preserved() {
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
    let left = grid.get(0, 0);
    let right = grid.get(3, 0);

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
fn test_sparse_stars_rejected() {
    // Simulate astronomical image: mostly background (100) with sparse bright stars
    let width = 128;
    let height = 128;
    let mut data = vec![100.0; width * height];

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
                data[ny * width + nx] = 500.0 + (dx.abs() + dy.abs()) as f32 * -100.0;
            }
        }
    }

    let pixels = Buffer2::new(width, height, data);
    let grid = make_grid(&pixels, 64);

    // All tiles should have median close to background (100)
    for ty in 0..grid.tiles_y() {
        for tx in 0..grid.tiles_x() {
            let stats = grid.get(tx, ty);
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
fn test_mask_excludes_sources_correctly() {
    // Background 50, sources at 200
    let width = 64;
    let height = 64;
    let mut data = vec![50.0; width * height];

    // Add bright source in top-left quadrant
    for y in 0..32 {
        for x in 0..32 {
            data[y * width + x] = 200.0;
        }
    }

    let pixels = Buffer2::new(width, height, data);

    // Mask the bright source
    let mut mask = BitBuffer2::new_filled(width, height, false);
    for y in 0..32 {
        for x in 0..32 {
            mask.set_xy(x, y, true);
        }
    }

    let grid = make_grid_with_mask(&pixels, 32, &mask);

    // Top-left tile (0,0): all pixels masked → falls back to all pixels (200.0)
    // Bottom-right tile (1,1): no masked pixels → uses background value
    let br = grid.get(1, 1);
    assert!(
        (br.sky - 50.0).abs() < 5.0,
        "Unmasked tile median {} should be ~50",
        br.sky
    );
}

// --- compute_y_spline_derivatives ---

#[test]
fn test_y_spline_derivatives_uniform_data() {
    // Uniform image → all medians equal → d2y = 0 everywhere
    let pixels = create_uniform_image(128, 128, 0.5);
    let grid = make_grid(&pixels, 32);

    for ty in 0..grid.tiles_y() {
        for tx in 0..grid.tiles_x() {
            assert!(
                grid.d2y_sky(tx, ty).abs() < 1e-6,
                "d2y_sky({},{}) = {}, expected 0",
                tx,
                ty,
                grid.d2y_sky(tx, ty)
            );
            assert!(
                grid.d2y_sigma(tx, ty).abs() < 1e-6,
                "d2y_sigma({},{}) = {}, expected 0",
                tx,
                ty,
                grid.d2y_sigma(tx, ty)
            );
        }
    }
}

#[test]
fn test_y_spline_derivatives_single_row() {
    // Single row of tiles → d2y = 0 (no Y interpolation)
    let pixels = create_uniform_image(128, 32, 0.5);
    let grid = make_grid(&pixels, 32);

    assert_eq!(grid.tiles_y(), 1);
    for tx in 0..grid.tiles_x() {
        assert_eq!(grid.d2y_sky(tx, 0), 0.0);
        assert_eq!(grid.d2y_sigma(tx, 0), 0.0);
    }
}

#[test]
fn test_y_spline_derivatives_two_rows() {
    // Two rows of tiles → natural spline gives d2 = 0 at both endpoints
    let width = 64;
    let height = 64;
    let data: Vec<f32> = (0..height)
        .flat_map(|y| std::iter::repeat_n(y as f32 / height as f32, width))
        .collect();
    let pixels = Buffer2::new(width, height, data);
    let grid = make_grid(&pixels, 32);

    assert_eq!(grid.tiles_y(), 2);
    for tx in 0..grid.tiles_x() {
        assert_eq!(grid.d2y_sky(tx, 0), 0.0);
        assert_eq!(grid.d2y_sky(tx, 1), 0.0);
    }
}

#[test]
fn test_y_spline_derivatives_natural_bc() {
    // With >= 3 rows, boundary d2 values should be 0 (natural BC)
    let width = 64;
    let height = 128;
    let data: Vec<f32> = (0..height)
        .flat_map(|y| std::iter::repeat_n(y as f32 / height as f32, width))
        .collect();
    let pixels = Buffer2::new(width, height, data);
    let grid = make_grid(&pixels, 32);

    assert_eq!(grid.tiles_y(), 4);
    for tx in 0..grid.tiles_x() {
        assert!(
            grid.d2y_sky(tx, 0).abs() < 1e-6,
            "Natural BC: d2y[{},0] = {}",
            tx,
            grid.d2y_sky(tx, 0)
        );
        assert!(
            grid.d2y_sky(tx, 3).abs() < 1e-6,
            "Natural BC: d2y[{},3] = {}",
            tx,
            grid.d2y_sky(tx, 3)
        );
    }
}

#[test]
fn test_y_spline_derivatives_quadratic_gradient() {
    // Create image where each row of tiles has quadratic Y values:
    // f(y) = y² → tile medians should approximate y_center²
    // With 4 tile rows (uniform h=32), same as test_solve_d2_quadratic_data
    // but through the full pipeline.
    let width = 64;
    let height = 128; // 4 tile rows of 32
    let data: Vec<f32> = (0..height)
        .flat_map(|y| {
            let val = (y as f32 / height as f32).powi(2); // [0, 1)
            std::iter::repeat_n(val, width)
        })
        .collect();

    let pixels = Buffer2::new(width, height, data);
    let grid = make_grid(&pixels, 32);

    assert_eq!(grid.tiles_y(), 4);

    // Natural BC: endpoints should be 0
    for tx in 0..grid.tiles_x() {
        assert!(
            grid.d2y_sky(tx, 0).abs() < 1e-5,
            "d2y[{},0] = {}, expected 0",
            tx,
            grid.d2y_sky(tx, 0)
        );
        assert!(
            grid.d2y_sky(tx, 3).abs() < 1e-5,
            "d2y[{},3] = {}, expected 0",
            tx,
            grid.d2y_sky(tx, 3)
        );
    }

    // Interior d2 should be nonzero (positive, since f''(y²) > 0)
    for tx in 0..grid.tiles_x() {
        assert!(
            grid.d2y_sky(tx, 1) > 1e-4,
            "d2y[{},1] = {}, expected positive",
            tx,
            grid.d2y_sky(tx, 1)
        );
        assert!(
            grid.d2y_sky(tx, 2) > 1e-4,
            "d2y[{},2] = {}, expected positive",
            tx,
            grid.d2y_sky(tx, 2)
        );
    }

    // All columns should have the same d2 values (uniform X data)
    if grid.tiles_x() >= 2 {
        for ty in 0..grid.tiles_y() {
            let d0 = grid.d2y_sky(0, ty);
            for tx in 1..grid.tiles_x() {
                assert!(
                    (grid.d2y_sky(tx, ty) - d0).abs() < 1e-5,
                    "d2y[{},{}] = {} != d2y[0,{}] = {}",
                    tx,
                    ty,
                    grid.d2y_sky(tx, ty),
                    ty,
                    d0
                );
            }
        }
    }
}

#[test]
fn test_photutils_sextractor_comparison() {
    // Test case similar to photutils/SExtractor documentation examples
    // Background level 1000 with noise sigma ~10
    let width = 256;
    let height = 256;

    // Generate pseudo-random noise using deterministic pattern
    let data: Vec<f32> = (0..width * height)
        .map(|i| {
            let noise = ((i * 7919 + 104729) % 1000) as f32 / 100.0 - 5.0; // -5 to +5
            1000.0 + noise * 2.0 // background 1000, noise ~10
        })
        .collect();

    let pixels = Buffer2::new(width, height, data);
    let grid = make_grid(&pixels, 64);

    // Check all tiles have reasonable background estimate
    for ty in 0..grid.tiles_y() {
        for tx in 0..grid.tiles_x() {
            let stats = grid.get(tx, ty);
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
