//! Tests for multi-threshold deblending.

use super::*;
use crate::star_detection::candidate_detection::label_map_from_raw;

/// Create a test image with Gaussian stars and return pixels, labels, and component data.
fn make_test_component(
    width: usize,
    height: usize,
    stars: &[(usize, usize, f32, f32)], // (cx, cy, amplitude, sigma)
) -> (Buffer2<f32>, LabelMap, ComponentData) {
    let mut pixels = Buffer2::new_filled(width, height, 0.0f32);
    let mut labels = Buffer2::new_filled(width, height, 0u32);

    let mut bbox = Aabb::empty();
    let mut area = 0;

    for (cx, cy, amplitude, sigma) in stars {
        let radius = (sigma * 4.0).ceil() as i32;

        for dy in -radius..=radius {
            for dx in -radius..=radius {
                let x = (*cx as i32 + dx) as usize;
                let y = (*cy as i32 + dy) as usize;

                if x < width && y < height {
                    let r2 = (dx * dx + dy * dy) as f32;
                    let value = amplitude * (-r2 / (2.0 * sigma * sigma)).exp();
                    if value > 0.001 {
                        pixels[(x, y)] += value;
                        if labels[(x, y)] == 0 {
                            labels[(x, y)] = 1;
                            bbox.include(Vec2us::new(x, y));
                            area += 1;
                        }
                    }
                }
            }
        }
    }

    let label_map = label_map_from_raw(labels, 1);
    let component = ComponentData {
        bbox,
        label: 1,
        area,
    };

    (pixels, label_map, component)
}

#[test]
fn test_single_star_no_deblending() {
    let (pixels, labels, data) = make_test_component(100, 100, &[(50, 50, 1.0, 3.0)]);
    let config = DeblendConfig::default();

    let result = deblend_multi_threshold(&data, &pixels, &labels, &config);

    assert_eq!(result.len(), 1, "Single star should produce one object");
    assert!((result[0].peak.x as i32 - 50).abs() <= 1);
    assert!((result[0].peak.y as i32 - 50).abs() <= 1);
}

#[test]
fn test_two_separated_stars_deblend() {
    let (pixels, labels, data) =
        make_test_component(100, 100, &[(30, 50, 1.0, 2.5), (70, 50, 0.8, 2.5)]);

    let config = DeblendConfig {
        n_thresholds: 32,
        min_contrast: 0.005,
        min_separation: 3,
        ..Default::default()
    };

    let result = deblend_multi_threshold(&data, &pixels, &labels, &config);

    assert_eq!(
        result.len(),
        2,
        "Two separated stars should produce two objects"
    );

    let mut peaks: Vec<_> = result.iter().map(|o| (o.peak.x, o.peak.y)).collect();
    peaks.sort_by_key(|&(x, _)| x);

    assert!(
        (peaks[0].0 as i32 - 30).abs() <= 2,
        "First peak should be near x=30"
    );
    assert!(
        (peaks[1].0 as i32 - 70).abs() <= 2,
        "Second peak should be near x=70"
    );
}

#[test]
fn test_faint_secondary_below_contrast() {
    let (pixels, labels, data) =
        make_test_component(100, 100, &[(30, 50, 1.0, 2.5), (70, 50, 0.001, 2.5)]);

    let config = DeblendConfig {
        n_thresholds: 32,
        min_contrast: 0.01,
        min_separation: 3,
        ..Default::default()
    };

    let result = deblend_multi_threshold(&data, &pixels, &labels, &config);

    assert_eq!(
        result.len(),
        1,
        "Faint secondary should not cause deblending"
    );
}

#[test]
fn test_threshold_levels_exponential() {
    let low = 0.1f32;
    let high = 1.0f32;
    let n = 10usize;
    let ratio = (high / low).max(1.0);
    let thresholds: Vec<f32> = (0..=n)
        .map(|i| {
            let t = i as f32 / n as f32;
            low * ratio.powf(t)
        })
        .collect();

    assert_eq!(thresholds.len(), 11);
    assert!((thresholds[0] - 0.1).abs() < 1e-6);
    assert!((thresholds[10] - 1.0).abs() < 1e-6);

    for i in 1..thresholds.len() {
        let step_ratio = thresholds[i] / thresholds[i - 1];
        let expected_ratio = (1.0f32 / 0.1).powf(1.0 / 10.0);
        assert!(
            (step_ratio - expected_ratio).abs() < 0.01,
            "Threshold spacing should be exponential"
        );
    }
}

#[test]
fn test_close_peaks_merge() {
    let (pixels, labels, data) =
        make_test_component(100, 100, &[(48, 50, 1.0, 2.0), (52, 50, 0.9, 2.0)]);

    let config = DeblendConfig {
        n_thresholds: 32,
        min_contrast: 0.005,
        min_separation: 5,
        ..Default::default()
    };

    let result = deblend_multi_threshold(&data, &pixels, &labels, &config);

    assert_eq!(result.len(), 1, "Close peaks should not be deblended");
}

#[test]
fn test_empty_component() {
    let pixels = Buffer2::new_filled(10, 10, 0.0f32);
    let labels_buf = Buffer2::new_filled(10, 10, 0u32);
    let labels = label_map_from_raw(labels_buf, 0);
    let data = ComponentData {
        bbox: Aabb::default(),
        label: 1,
        area: 0,
    };
    let config = DeblendConfig::default();

    let result = deblend_multi_threshold(&data, &pixels, &labels, &config);

    assert!(result.is_empty());
}

#[test]
fn test_deblend_disabled_with_high_contrast() {
    let (pixels, labels, data) =
        make_test_component(100, 100, &[(30, 50, 1.0, 2.5), (70, 50, 0.8, 2.5)]);

    let config = DeblendConfig {
        n_thresholds: 32,
        min_contrast: 1.0,
        min_separation: 3,
        ..Default::default()
    };

    let result = deblend_multi_threshold(&data, &pixels, &labels, &config);

    assert_eq!(
        result.len(),
        1,
        "High contrast setting should disable deblending"
    );
}

#[test]
fn test_three_stars_deblend() {
    let (pixels, labels, data) = make_test_component(
        150,
        100,
        &[(30, 50, 1.0, 2.5), (75, 50, 0.9, 2.5), (120, 50, 0.8, 2.5)],
    );

    let config = DeblendConfig {
        n_thresholds: 32,
        min_contrast: 0.005,
        min_separation: 3,
        ..Default::default()
    };

    let result = deblend_multi_threshold(&data, &pixels, &labels, &config);

    assert_eq!(
        result.len(),
        3,
        "Three separated stars should produce three objects"
    );

    let mut peaks: Vec<_> = result.iter().map(|o| o.peak.x).collect();
    peaks.sort();
    assert!((peaks[0] as i32 - 30).abs() <= 2);
    assert!((peaks[1] as i32 - 75).abs() <= 2);
    assert!((peaks[2] as i32 - 120).abs() <= 2);
}

#[test]
fn test_hierarchical_deblend() {
    let (pixels, labels, data) = make_test_component(
        150,
        100,
        &[(30, 50, 1.0, 2.5), (100, 50, 0.8, 2.5), (115, 50, 0.7, 2.5)],
    );

    let config = DeblendConfig {
        n_thresholds: 32,
        min_contrast: 0.1,
        min_separation: 3,
        ..Default::default()
    };

    let result = deblend_multi_threshold(&data, &pixels, &labels, &config);

    assert!(
        result.len() >= 2,
        "Should find at least the isolated star and close pair"
    );
}

#[test]
fn test_equal_brightness_stars() {
    let (pixels, labels, data) =
        make_test_component(100, 100, &[(30, 50, 1.0, 2.5), (70, 50, 1.0, 2.5)]);

    let config = DeblendConfig {
        n_thresholds: 32,
        min_contrast: 0.005,
        min_separation: 3,
        ..Default::default()
    };

    let result = deblend_multi_threshold(&data, &pixels, &labels, &config);

    assert_eq!(
        result.len(),
        2,
        "Equal brightness stars should both be found"
    );

    let area_diff = (result[0].area as i32 - result[1].area as i32).abs();
    let avg_area = (result[0].area + result[1].area) / 2;
    assert!(
        area_diff < (avg_area as i32 / 2),
        "Equal stars should have similar areas"
    );
}

#[test]
fn test_contrast_at_boundary() {
    let (pixels, labels, data) =
        make_test_component(100, 100, &[(30, 50, 1.0, 2.5), (70, 50, 0.1, 2.5)]);

    let config_pass = DeblendConfig {
        n_thresholds: 32,
        min_contrast: 0.09,
        min_separation: 3,
        ..Default::default()
    };
    let result_pass = deblend_multi_threshold(&data, &pixels, &labels, &config_pass);

    let config_fail = DeblendConfig {
        n_thresholds: 32,
        min_contrast: 0.15,
        min_separation: 3,
        ..Default::default()
    };
    let result_fail = deblend_multi_threshold(&data, &pixels, &labels, &config_fail);

    assert!(
        result_pass.len() >= result_fail.len(),
        "Lower contrast threshold should find more or equal objects"
    );
}

#[test]
fn test_pixel_assignment_conservation() {
    let (pixels, labels, data) =
        make_test_component(100, 100, &[(30, 50, 1.0, 2.5), (70, 50, 0.8, 2.5)]);

    let config = DeblendConfig {
        n_thresholds: 32,
        min_contrast: 0.005,
        min_separation: 3,
        ..Default::default()
    };

    let result = deblend_multi_threshold(&data, &pixels, &labels, &config);

    let total_area: usize = result.iter().map(|o| o.area).sum();
    assert_eq!(
        total_area, data.area,
        "Sum of deblended areas should equal original area"
    );
}

#[test]
fn test_vertical_star_pair() {
    let (pixels, labels, data) =
        make_test_component(100, 100, &[(50, 30, 1.0, 2.5), (50, 70, 0.8, 2.5)]);

    let config = DeblendConfig {
        n_thresholds: 32,
        min_contrast: 0.005,
        min_separation: 3,
        ..Default::default()
    };

    let result = deblend_multi_threshold(&data, &pixels, &labels, &config);

    assert_eq!(result.len(), 2, "Vertically separated stars should deblend");

    let mut peaks: Vec<_> = result.iter().map(|o| o.peak.y).collect();
    peaks.sort();
    assert!((peaks[0] as i32 - 30).abs() <= 2);
    assert!((peaks[1] as i32 - 70).abs() <= 2);
}

#[test]
fn test_diagonal_star_pair() {
    let (pixels, labels, data) =
        make_test_component(100, 100, &[(30, 30, 1.0, 2.5), (70, 70, 0.8, 2.5)]);

    let config = DeblendConfig {
        n_thresholds: 32,
        min_contrast: 0.005,
        min_separation: 3,
        ..Default::default()
    };

    let result = deblend_multi_threshold(&data, &pixels, &labels, &config);

    assert_eq!(result.len(), 2, "Diagonally separated stars should deblend");
}

#[test]
fn test_n_thresholds_effect() {
    let (pixels, labels, data) =
        make_test_component(100, 100, &[(35, 50, 1.0, 2.5), (65, 50, 0.9, 2.5)]);

    let config_few = DeblendConfig {
        n_thresholds: 4,
        min_contrast: 0.005,
        min_separation: 3,
        ..Default::default()
    };

    let config_many = DeblendConfig {
        n_thresholds: 64,
        min_contrast: 0.005,
        min_separation: 3,
        ..Default::default()
    };

    let result_few = deblend_multi_threshold(&data, &pixels, &labels, &config_few);
    let result_many = deblend_multi_threshold(&data, &pixels, &labels, &config_many);

    assert!(
        result_many.len() >= result_few.len(),
        "More thresholds should find >= objects"
    );
}

#[test]
fn test_single_pixel_component() {
    let mut pixels = Buffer2::new_filled(10, 10, 0.0f32);
    let mut labels_buf = Buffer2::new_filled(10, 10, 0u32);

    pixels[(5, 5)] = 1.0;
    labels_buf[(5, 5)] = 1;

    let labels = label_map_from_raw(labels_buf, 1);
    let data = ComponentData {
        bbox: Aabb::new(Vec2us::new(5, 5), Vec2us::new(5, 5)),
        label: 1,
        area: 1,
    };

    let config = DeblendConfig::default();
    let result = deblend_multi_threshold(&data, &pixels, &labels, &config);

    assert_eq!(result.len(), 1, "Single pixel should produce one object");
    assert_eq!(result[0].area, 1);
}

#[test]
fn test_flat_profile_no_deblend() {
    let mut pixels = Buffer2::new_filled(50, 50, 0.0f32);
    let mut labels_buf = Buffer2::new_filled(50, 50, 0u32);

    let mut bbox = Aabb::empty();
    let mut area = 0;

    for y in 10..40 {
        for x in 10..40 {
            let dx = x as i32 - 25;
            let dy = y as i32 - 25;
            if dx * dx + dy * dy < 150 {
                pixels[(x, y)] = 1.0;
                labels_buf[(x, y)] = 1;
                bbox.include(Vec2us::new(x, y));
                area += 1;
            }
        }
    }

    let labels = label_map_from_raw(labels_buf, 1);
    let data = ComponentData {
        bbox,
        label: 1,
        area,
    };

    let config = DeblendConfig {
        n_thresholds: 32,
        min_contrast: 0.005,
        min_separation: 3,
        ..Default::default()
    };

    let result = deblend_multi_threshold(&data, &pixels, &labels, &config);

    assert_eq!(
        result.len(),
        1,
        "Flat profile should not deblend into multiple objects"
    );
}

// ========================================================================
// Additional tests for optimized code paths
// ========================================================================

#[test]
fn test_many_stars_max_peaks_limit() {
    // Create more stars than MAX_PEAKS to test limiting behavior
    let stars: Vec<_> = (0..12)
        .map(|i| (15 + i * 12, 50usize, 1.0 - i as f32 * 0.05, 2.0f32))
        .collect();

    let (pixels, labels, data) = make_test_component(180, 100, &stars);

    let config = DeblendConfig {
        n_thresholds: 32,
        min_contrast: 0.005,
        min_separation: 3,
        ..Default::default()
    };

    let result = deblend_multi_threshold(&data, &pixels, &labels, &config);

    // Should not exceed MAX_PEAKS
    assert!(
        result.len() <= MAX_PEAKS,
        "Should not exceed MAX_PEAKS ({}), got {}",
        MAX_PEAKS,
        result.len()
    );

    // Area should still be conserved
    let total_area: usize = result.iter().map(|o| o.area).sum();
    assert_eq!(total_area, data.area, "Area should be conserved");
}

#[test]
fn test_large_tree_over_64_nodes() {
    // Create a scenario that might produce > 64 nodes to test HashSet fallback
    // Use many small stars that will create many tree nodes
    let mut stars = Vec::new();
    for row in 0..4 {
        for col in 0..4 {
            let x = 20 + col * 25;
            let y = 20 + row * 25;
            let amp = 1.0 - (row * 4 + col) as f32 * 0.03;
            stars.push((x, y, amp, 2.0f32));
        }
    }

    let (pixels, labels, data) = make_test_component(150, 150, &stars);

    let config = DeblendConfig {
        n_thresholds: 64, // More thresholds = more potential nodes
        min_contrast: 0.005,
        min_separation: 3,
        ..Default::default()
    };

    let result = deblend_multi_threshold(&data, &pixels, &labels, &config);

    // Should find multiple objects
    assert!(result.len() >= 2, "Should find multiple objects");

    // Area conservation
    let total_area: usize = result.iter().map(|o| o.area).sum();
    assert_eq!(total_area, data.area, "Area should be conserved");
}

#[test]
fn test_very_large_tree_heap_fallback() {
    // Create a scenario that produces > 128 nodes to test heap fallback path
    // Use many stars in a grid pattern with high threshold count
    let mut stars = Vec::new();
    for row in 0..6 {
        for col in 0..6 {
            let x = 15 + col * 20;
            let y = 15 + row * 20;
            let amp = 1.0 - (row * 6 + col) as f32 * 0.02;
            stars.push((x, y, amp, 1.8f32));
        }
    }

    let (pixels, labels, data) = make_test_component(150, 150, &stars);

    let config = DeblendConfig {
        n_thresholds: 128, // Very high threshold count to create many nodes
        min_contrast: 0.001,
        min_separation: 2,
        ..Default::default()
    };

    let result = deblend_multi_threshold(&data, &pixels, &labels, &config);

    // Should find multiple objects (exact count depends on merging)
    assert!(!result.is_empty(), "Should find at least one object");

    // Area conservation
    let total_area: usize = result.iter().map(|o| o.area).sum();
    assert_eq!(total_area, data.area, "Area should be conserved");
}

#[test]
fn test_buffer_reuse_consistency() {
    // Run the same deblending multiple times to ensure buffer reuse doesn't cause issues
    let (pixels, labels, data) =
        make_test_component(100, 100, &[(30, 50, 1.0, 2.5), (70, 50, 0.8, 2.5)]);

    let config = DeblendConfig {
        n_thresholds: 32,
        min_contrast: 0.005,
        min_separation: 3,
        ..Default::default()
    };

    let result1 = deblend_multi_threshold(&data, &pixels, &labels, &config);
    let result2 = deblend_multi_threshold(&data, &pixels, &labels, &config);
    let result3 = deblend_multi_threshold(&data, &pixels, &labels, &config);

    // All runs should produce identical results
    assert_eq!(result1.len(), result2.len());
    assert_eq!(result2.len(), result3.len());

    for i in 0..result1.len() {
        assert_eq!(result1[i].peak, result2[i].peak);
        assert_eq!(result2[i].peak, result3[i].peak);
        assert_eq!(result1[i].area, result2[i].area);
        assert_eq!(result2[i].area, result3[i].area);
    }
}

#[test]
fn test_connected_regions_complex_shape() {
    // Test with a dumbbell-shaped component (two blobs connected by thin bridge)
    // The two blobs have distinct peaks that should be deblended
    let (pixels, labels, data) = make_test_component(
        100,
        50,
        &[
            (20, 25, 1.0, 3.0), // Left blob
            (80, 25, 0.9, 3.0), // Right blob
        ],
    );

    let config = DeblendConfig {
        n_thresholds: 32,
        min_contrast: 0.005,
        min_separation: 3,
        ..Default::default()
    };

    let result = deblend_multi_threshold(&data, &pixels, &labels, &config);

    // Should find both peaks
    assert_eq!(result.len(), 2, "Should find both peaks");

    // Area conservation
    let total_area: usize = result.iter().map(|o| o.area).sum();
    assert_eq!(total_area, data.area, "Area should be conserved");

    // Peaks should be at expected positions
    let mut peak_xs: Vec<_> = result.iter().map(|c| c.peak.x).collect();
    peak_xs.sort();
    assert!((peak_xs[0] as i32 - 20).abs() <= 1);
    assert!((peak_xs[1] as i32 - 80).abs() <= 1);
}

#[test]
fn test_bbox_contains_all_peaks() {
    // Verify that each deblended object's bbox contains its peak
    let (pixels, labels, data) = make_test_component(
        150,
        100,
        &[(30, 30, 1.0, 2.5), (75, 50, 0.9, 2.5), (120, 70, 0.8, 2.5)],
    );

    let config = DeblendConfig {
        n_thresholds: 32,
        min_contrast: 0.005,
        min_separation: 3,
        ..Default::default()
    };

    let result = deblend_multi_threshold(&data, &pixels, &labels, &config);

    for candidate in &result {
        assert!(
            candidate.bbox.contains(candidate.peak),
            "Candidate bbox {:?} should contain peak {:?}",
            candidate.bbox,
            candidate.peak
        );
    }
}

#[test]
fn test_peak_values_match_image() {
    // Verify that peak_value matches the actual pixel value
    let (pixels, labels, data) =
        make_test_component(100, 100, &[(30, 50, 1.0, 2.5), (70, 50, 0.8, 2.5)]);

    let config = DeblendConfig {
        n_thresholds: 32,
        min_contrast: 0.005,
        min_separation: 3,
        ..Default::default()
    };

    let result = deblend_multi_threshold(&data, &pixels, &labels, &config);

    for candidate in &result {
        let actual_value = pixels[(candidate.peak.x, candidate.peak.y)];
        assert!(
            (candidate.peak_value - actual_value).abs() < 1e-6,
            "peak_value {} should match pixel value {}",
            candidate.peak_value,
            actual_value
        );
    }
}

#[test]
fn test_single_threshold_level() {
    // Test with n_thresholds = 1 (edge case)
    let (pixels, labels, data) =
        make_test_component(100, 100, &[(30, 50, 1.0, 2.5), (70, 50, 0.8, 2.5)]);

    let config = DeblendConfig {
        n_thresholds: 1,
        min_contrast: 0.005,
        min_separation: 3,
        ..Default::default()
    };

    let result = deblend_multi_threshold(&data, &pixels, &labels, &config);

    // Should still produce valid output
    assert!(!result.is_empty(), "Should produce at least one object");

    // Area conservation
    let total_area: usize = result.iter().map(|o| o.area).sum();
    assert_eq!(total_area, data.area, "Area should be conserved");
}

#[test]
fn test_zero_threshold_level() {
    // Test with n_thresholds = 0 (edge case - should still work)
    let (pixels, labels, data) = make_test_component(100, 100, &[(50, 50, 1.0, 2.5)]);

    let config = DeblendConfig {
        n_thresholds: 0,
        min_contrast: 0.005,
        min_separation: 3,
        ..Default::default()
    };

    let result = deblend_multi_threshold(&data, &pixels, &labels, &config);

    // Should produce single object
    assert_eq!(result.len(), 1, "Should produce one object");
    assert_eq!(result[0].area, data.area);
}

// ========================================================================
// Unit tests for PixelGrid (bit-packed visited flags)
// ========================================================================

#[test]
fn test_pixel_grid_empty() {
    let grid = PixelGrid::empty();
    assert_eq!(grid.width, 0);
    assert_eq!(grid.height, 0);
    assert!(grid.is_visited(0, 0)); // Out of bounds treated as visited
}

#[test]
fn test_pixel_grid_basic_operations() {
    let pixels = vec![
        Pixel {
            pos: Vec2us::new(10, 10),
            value: 1.0,
        },
        Pixel {
            pos: Vec2us::new(11, 10),
            value: 2.0,
        },
        Pixel {
            pos: Vec2us::new(10, 11),
            value: 3.0,
        },
    ];

    let mut grid = PixelGrid::empty();
    grid.reset_with_pixels(&pixels);

    // Check grid dimensions are set correctly
    assert!(grid.width > 0);
    assert!(grid.height > 0);

    // Check visited flags start as false for pixel positions
    assert!(!grid.is_visited(10, 10));
    assert!(!grid.is_visited(11, 10));
    assert!(!grid.is_visited(10, 11));

    // Mark visited and check
    assert!(grid.mark_visited(10, 10)); // First mark returns true
    assert!(!grid.mark_visited(10, 10)); // Second mark returns false
    assert!(grid.is_visited(10, 10));
    assert!(!grid.is_visited(11, 10)); // Other positions unaffected
}

#[test]
fn test_pixel_grid_connected_regions() {
    // Test that pixel values are stored correctly by using find_connected_regions_grid
    // which exercises the actual code path including value lookups
    let pixels = vec![
        Pixel {
            pos: Vec2us::new(10, 10),
            value: 1.0,
        },
        Pixel {
            pos: Vec2us::new(11, 10),
            value: 2.0,
        },
        Pixel {
            pos: Vec2us::new(10, 11),
            value: 3.0,
        },
    ];

    let mut grid = PixelGrid::empty();
    let mut queue = Vec::new();
    let mut regions = Vec::new();

    find_connected_regions_grid(&pixels, &mut regions, &mut grid, &mut queue);

    // All 3 pixels should be in one connected region (they're adjacent)
    assert_eq!(regions.len(), 1);
    assert_eq!(regions[0].len(), 3);

    // Verify the values were preserved
    let values: std::collections::HashSet<_> = regions[0].iter().map(|p| p.value as i32).collect();
    assert!(values.contains(&1));
    assert!(values.contains(&2));
    assert!(values.contains(&3));
}

#[test]
fn test_pixel_grid_bit_packing() {
    // Test that bit packing works correctly across word boundaries
    // Create a grid wider than 64 pixels to span multiple u64 words
    let mut pixels = Vec::new();
    for x in 0..100 {
        pixels.push(Pixel {
            pos: Vec2us::new(x, 50),
            value: x as f32,
        });
    }

    let mut grid = PixelGrid::empty();
    grid.reset_with_pixels(&pixels);

    // Mark every other pixel as visited
    for x in (0..100).step_by(2) {
        assert!(grid.mark_visited(x, 50));
    }

    // Verify the pattern
    for x in 0..100 {
        if x % 2 == 0 {
            assert!(grid.is_visited(x, 50), "Pixel at x={} should be visited", x);
        } else {
            assert!(
                !grid.is_visited(x, 50),
                "Pixel at x={} should not be visited",
                x
            );
        }
    }
}

#[test]
fn test_pixel_grid_boundary_conditions() {
    let pixels = vec![Pixel {
        pos: Vec2us::new(5, 5),
        value: 1.0,
    }];

    let mut grid = PixelGrid::empty();
    grid.reset_with_pixels(&pixels);

    // Out of bounds should be treated as visited (can't mark)
    assert!(grid.is_visited(0, 0)); // Out of bounds treated as visited
    assert!(grid.is_visited(100, 100));
    assert!(!grid.mark_visited(0, 0)); // Can't mark out of bounds
    assert!(!grid.mark_visited(100, 100));

    // Boundary pixels (1-pixel border) should work
    // With offset at min-1, the grid includes positions around the pixel
    assert!(!grid.is_visited(4, 5)); // Within grid border
    assert!(grid.mark_visited(4, 5));
    assert!(grid.is_visited(4, 5));
}

#[test]
fn test_pixel_grid_reuse() {
    let mut grid = PixelGrid::empty();

    // First use
    let pixels1 = vec![
        Pixel {
            pos: Vec2us::new(10, 10),
            value: 1.0,
        },
        Pixel {
            pos: Vec2us::new(15, 15),
            value: 2.0,
        },
    ];
    grid.reset_with_pixels(&pixels1);
    grid.mark_visited(10, 10);

    // Reuse with different pixels - should reset visited flags
    let pixels2 = vec![
        Pixel {
            pos: Vec2us::new(20, 20),
            value: 3.0,
        },
        Pixel {
            pos: Vec2us::new(25, 25),
            value: 4.0,
        },
    ];
    grid.reset_with_pixels(&pixels2);

    // New positions should not be visited (flags reset)
    assert!(!grid.is_visited(20, 20));
    assert!(!grid.is_visited(25, 25));

    // Test that the new pixels are properly stored via connected regions
    let mut queue = Vec::new();
    let mut regions = Vec::new();
    find_connected_regions_grid(&pixels2, &mut regions, &mut grid, &mut queue);

    // Two separate pixels should form two regions (not adjacent)
    assert_eq!(regions.len(), 2);
}

#[test]
fn test_pixel_grid_single_pixel() {
    let pixels = vec![Pixel {
        pos: Vec2us::new(50, 50),
        value: 42.0,
    }];

    let mut grid = PixelGrid::empty();
    grid.reset_with_pixels(&pixels);

    assert!(!grid.is_visited(50, 50));
    assert!(grid.mark_visited(50, 50));
    assert!(grid.is_visited(50, 50));

    // Verify value storage via connected regions
    let mut queue = Vec::new();
    let mut regions = Vec::new();
    grid.reset_with_pixels(&pixels); // Reset to clear visited flags
    find_connected_regions_grid(&pixels, &mut regions, &mut grid, &mut queue);

    assert_eq!(regions.len(), 1);
    assert_eq!(regions[0].len(), 1);
    assert_eq!(regions[0][0].value, 42.0);
}

#[test]
fn test_pixel_grid_empty_pixels() {
    let mut grid = PixelGrid::empty();
    grid.reset_with_pixels(&[]);

    assert_eq!(grid.width, 0);
    assert_eq!(grid.height, 0);
    assert!(grid.is_visited(0, 0)); // Empty grid treats all as visited
}

#[test]
fn test_pixel_grid_generation_counter_multiple_resets() {
    // Test that generation counter correctly resets visited flags across many resets
    let pixels = vec![
        Pixel {
            pos: Vec2us::new(10, 10),
            value: 1.0,
        },
        Pixel {
            pos: Vec2us::new(11, 10),
            value: 2.0,
        },
    ];

    let mut grid = PixelGrid::empty();

    // Perform many reset cycles to exercise the generation counter
    for i in 0..100 {
        grid.reset_with_pixels(&pixels);

        // After reset, nothing should be visited
        assert!(
            !grid.is_visited(10, 10),
            "Iteration {}: position should not be visited after reset",
            i
        );
        assert!(!grid.is_visited(11, 10));

        // Mark one position
        assert!(grid.mark_visited(10, 10));
        assert!(grid.is_visited(10, 10));
        assert!(!grid.is_visited(11, 10));

        // Mark again should return false (already visited)
        assert!(!grid.mark_visited(10, 10));
    }
}

#[test]
fn test_pixel_grid_generation_counter_different_sizes() {
    // Test generation counter with varying grid sizes (exercises resize logic)
    let mut grid = PixelGrid::empty();

    // Small grid
    let small_pixels = vec![Pixel {
        pos: Vec2us::new(5, 5),
        value: 1.0,
    }];
    grid.reset_with_pixels(&small_pixels);
    grid.mark_visited(5, 5);
    assert!(grid.is_visited(5, 5));

    // Larger grid - generation counter should reset visited status
    let large_pixels: Vec<Pixel> = (0..50)
        .map(|i| Pixel {
            pos: Vec2us::new(i, i),
            value: i as f32,
        })
        .collect();
    grid.reset_with_pixels(&large_pixels);

    // All positions should be unvisited after reset, even those that overlap
    // with the previous smaller grid
    assert!(!grid.is_visited(5, 5));
    for i in 0..50 {
        assert!(
            !grid.is_visited(i, i),
            "Position ({}, {}) should not be visited",
            i,
            i
        );
    }

    // Back to smaller grid
    grid.reset_with_pixels(&small_pixels);
    assert!(!grid.is_visited(5, 5));
}

// ========================================================================
// Unit tests for NodeGrid
// ========================================================================

#[test]
fn test_node_grid_empty() {
    let grid = NodeGrid::empty();
    assert_eq!(grid.width, 0);
    assert_eq!(grid.height, 0);
    assert!(grid.get(0, 0).is_none());
}

#[test]
fn test_node_grid_basic_operations() {
    let pixels = vec![
        Pixel {
            pos: Vec2us::new(10, 10),
            value: 1.0,
        },
        Pixel {
            pos: Vec2us::new(11, 10),
            value: 2.0,
        },
        Pixel {
            pos: Vec2us::new(10, 11),
            value: 3.0,
        },
    ];

    let mut grid = NodeGrid::empty();
    grid.reset_with_pixels(&pixels);

    // Initially all positions should be unassigned
    assert!(grid.get(10, 10).is_none());
    assert!(grid.get(11, 10).is_none());

    // Set node indices
    grid.set(10, 10, 0);
    grid.set(11, 10, 1);
    grid.set(10, 11, 0);

    // Verify
    assert_eq!(grid.get(10, 10), Some(0));
    assert_eq!(grid.get(11, 10), Some(1));
    assert_eq!(grid.get(10, 11), Some(0));

    // Out of bounds should return None
    assert!(grid.get(100, 100).is_none());
}

#[test]
fn test_node_grid_overwrite() {
    let pixels = vec![Pixel {
        pos: Vec2us::new(5, 5),
        value: 1.0,
    }];

    let mut grid = NodeGrid::empty();
    grid.reset_with_pixels(&pixels);

    grid.set(5, 5, 10);
    assert_eq!(grid.get(5, 5), Some(10));

    // Overwrite with new value
    grid.set(5, 5, 20);
    assert_eq!(grid.get(5, 5), Some(20));
}

#[test]
fn test_node_grid_reuse() {
    let mut grid = NodeGrid::empty();

    // First use
    let pixels1 = vec![Pixel {
        pos: Vec2us::new(10, 10),
        value: 1.0,
    }];
    grid.reset_with_pixels(&pixels1);
    grid.set(10, 10, 5);
    assert_eq!(grid.get(10, 10), Some(5));

    // Reuse with different pixels
    let pixels2 = vec![Pixel {
        pos: Vec2us::new(20, 20),
        value: 2.0,
    }];
    grid.reset_with_pixels(&pixels2);

    // Old position should no longer be valid
    assert!(grid.get(10, 10).is_none());

    // New position should be unassigned
    assert!(grid.get(20, 20).is_none());
}

#[test]
fn test_node_grid_large_indices() {
    let pixels = vec![Pixel {
        pos: Vec2us::new(100, 100),
        value: 1.0,
    }];

    let mut grid = NodeGrid::empty();
    grid.reset_with_pixels(&pixels);

    // Test with large node index (but within u32 range)
    let large_idx = 1_000_000;
    grid.set(100, 100, large_idx);
    assert_eq!(grid.get(100, 100), Some(large_idx));
}

#[test]
fn test_node_grid_boundary() {
    let pixels = vec![
        Pixel {
            pos: Vec2us::new(0, 0),
            value: 1.0,
        },
        Pixel {
            pos: Vec2us::new(99, 99),
            value: 2.0,
        },
    ];

    let mut grid = NodeGrid::empty();
    grid.reset_with_pixels(&pixels);

    grid.set(0, 0, 1);
    grid.set(99, 99, 2);

    assert_eq!(grid.get(0, 0), Some(1));
    assert_eq!(grid.get(99, 99), Some(2));

    // Just outside the grid
    assert!(grid.get(100, 100).is_none());
}

// ========================================================================
// Integration tests for grid-based connected component finding
// ========================================================================

#[test]
fn test_find_connected_regions_grid_single_region() {
    // Create a small connected region
    let pixels = vec![
        Pixel {
            pos: Vec2us::new(5, 5),
            value: 1.0,
        },
        Pixel {
            pos: Vec2us::new(6, 5),
            value: 1.0,
        },
        Pixel {
            pos: Vec2us::new(5, 6),
            value: 1.0,
        },
        Pixel {
            pos: Vec2us::new(6, 6),
            value: 1.0,
        },
    ];

    let mut regions = Vec::new();
    let mut grid = PixelGrid::empty();
    let mut queue = Vec::new();

    find_connected_regions_grid(&pixels, &mut regions, &mut grid, &mut queue);

    assert_eq!(regions.len(), 1, "Should find one connected region");
    assert_eq!(regions[0].len(), 4, "Region should contain all 4 pixels");
}

#[test]
fn test_find_connected_regions_grid_two_regions() {
    // Create two separate regions
    let pixels = vec![
        // Region 1
        Pixel {
            pos: Vec2us::new(5, 5),
            value: 1.0,
        },
        Pixel {
            pos: Vec2us::new(6, 5),
            value: 1.0,
        },
        // Region 2 (far away)
        Pixel {
            pos: Vec2us::new(50, 50),
            value: 2.0,
        },
        Pixel {
            pos: Vec2us::new(51, 50),
            value: 2.0,
        },
    ];

    let mut regions = Vec::new();
    let mut grid = PixelGrid::empty();
    let mut queue = Vec::new();

    find_connected_regions_grid(&pixels, &mut regions, &mut grid, &mut queue);

    assert_eq!(regions.len(), 2, "Should find two separate regions");
    assert_eq!(
        regions[0].len() + regions[1].len(),
        4,
        "Total pixels should be 4"
    );
}

#[test]
fn test_find_connected_regions_grid_diagonal_connectivity() {
    // Test 8-connectivity (diagonals should connect)
    let pixels = vec![
        Pixel {
            pos: Vec2us::new(5, 5),
            value: 1.0,
        },
        Pixel {
            pos: Vec2us::new(6, 6),
            value: 1.0,
        }, // Diagonal neighbor
    ];

    let mut regions = Vec::new();
    let mut grid = PixelGrid::empty();
    let mut queue = Vec::new();

    find_connected_regions_grid(&pixels, &mut regions, &mut grid, &mut queue);

    assert_eq!(
        regions.len(),
        1,
        "Diagonal neighbors should be connected (8-connectivity)"
    );
    assert_eq!(regions[0].len(), 2);
}

#[test]
fn test_find_significant_branches_small_tree() {
    // Test find_significant_branches with a small tree (stack allocation path)
    use smallvec::SmallVec as SV;

    // Build a simple tree: root with 2 children
    let tree = vec![
        DeblendNode {
            peak: Pixel {
                pos: Vec2us::new(10, 10),
                value: 1.0,
            },
            flux: 100.0,
            children: SV::from_slice(&[1, 2]),
        },
        DeblendNode {
            peak: Pixel {
                pos: Vec2us::new(5, 5),
                value: 0.8,
            },
            flux: 40.0,
            children: SV::new(),
        },
        DeblendNode {
            peak: Pixel {
                pos: Vec2us::new(15, 15),
                value: 0.7,
            },
            flux: 35.0,
            children: SV::new(),
        },
    ];

    // With low contrast, both children should be returned as leaves
    let leaves = find_significant_branches(&tree, 0.1);
    assert_eq!(leaves.len(), 2);
    assert!(leaves.contains(&1));
    assert!(leaves.contains(&2));

    // With high contrast, children don't pass, root becomes leaf
    let leaves = find_significant_branches(&tree, 0.9);
    assert_eq!(leaves.len(), 1);
    assert!(leaves.contains(&0));
}

#[test]
fn test_find_significant_branches_heap_fallback() {
    // Test find_significant_branches with a tree > MAX_TREE_SIZE (heap allocation path)
    use smallvec::SmallVec as SV;

    // Build a large tree with > 128 nodes
    let mut tree = Vec::with_capacity(150);

    // Root node with many children
    let num_children = 140;
    let child_indices: SV<[usize; 8]> = (1..=8.min(num_children)).collect();

    tree.push(DeblendNode {
        peak: Pixel {
            pos: Vec2us::new(50, 50),
            value: 1.0,
        },
        flux: 1000.0,
        children: child_indices,
    });

    // Add many leaf nodes
    for i in 1..=num_children {
        tree.push(DeblendNode {
            peak: Pixel {
                pos: Vec2us::new(i, i),
                value: 0.5,
            },
            flux: 10.0,
            children: SV::new(),
        });
    }

    assert!(
        tree.len() > MAX_TREE_SIZE,
        "Tree should exceed MAX_TREE_SIZE"
    );

    // Should use heap fallback but still work correctly
    let leaves = find_significant_branches(&tree, 0.001);

    // With very low contrast, children pass and become leaves
    assert!(!leaves.is_empty());
}

#[test]
fn test_visit_neighbors_grid_all_directions() {
    // Create a cross pattern and verify all neighbors are visited
    let pixels = vec![
        Pixel {
            pos: Vec2us::new(10, 10),
            value: 1.0,
        }, // Center
        Pixel {
            pos: Vec2us::new(9, 9),
            value: 1.0,
        }, // Top-left
        Pixel {
            pos: Vec2us::new(10, 9),
            value: 1.0,
        }, // Top
        Pixel {
            pos: Vec2us::new(11, 9),
            value: 1.0,
        }, // Top-right
        Pixel {
            pos: Vec2us::new(9, 10),
            value: 1.0,
        }, // Left
        Pixel {
            pos: Vec2us::new(11, 10),
            value: 1.0,
        }, // Right
        Pixel {
            pos: Vec2us::new(9, 11),
            value: 1.0,
        }, // Bottom-left
        Pixel {
            pos: Vec2us::new(10, 11),
            value: 1.0,
        }, // Bottom
        Pixel {
            pos: Vec2us::new(11, 11),
            value: 1.0,
        }, // Bottom-right
    ];

    let mut regions = Vec::new();
    let mut grid = PixelGrid::empty();
    let mut queue = Vec::new();

    find_connected_regions_grid(&pixels, &mut regions, &mut grid, &mut queue);

    assert_eq!(regions.len(), 1, "All pixels should be in one region");
    assert_eq!(regions[0].len(), 9, "All 9 pixels should be found");
}
