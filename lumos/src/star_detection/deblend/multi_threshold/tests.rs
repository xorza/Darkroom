//! Tests for multi-threshold deblending.

use super::*;
use crate::star_detection::labeling::label_map_from_raw;

/// Convenience wrapper for tests — creates fresh buffers per call.
fn deblend_multi_threshold_test(
    data: &ComponentData,
    pixels: &Buffer2<f32>,
    labels: &LabelMap,
    n_thresholds: usize,
    min_separation: usize,
    min_contrast: f32,
) -> SmallVec<[Region; MAX_PEAKS]> {
    let mut buffers = DeblendBuffers::new();
    deblend_multi_threshold(
        data,
        pixels,
        labels,
        n_thresholds,
        min_separation,
        min_contrast,
        &mut buffers,
    )
}

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
    let result = deblend_multi_threshold_test(&data, &pixels, &labels, 32, 3, 0.005);

    assert_eq!(result.len(), 1, "Single star should produce one object");
    assert!((result[0].peak.x as i32 - 50).abs() <= 1);
    assert!((result[0].peak.y as i32 - 50).abs() <= 1);
}

#[test]
fn test_two_separated_stars_deblend() {
    let (pixels, labels, data) =
        make_test_component(100, 100, &[(30, 50, 1.0, 2.5), (70, 50, 0.8, 2.5)]);

    let result = deblend_multi_threshold_test(&data, &pixels, &labels, 32, 3, 0.005);

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

    let result = deblend_multi_threshold_test(&data, &pixels, &labels, 32, 3, 0.01);

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

    let result = deblend_multi_threshold_test(&data, &pixels, &labels, 32, 5, 0.005);

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
    let result = deblend_multi_threshold_test(&data, &pixels, &labels, 32, 3, 0.005);

    assert!(result.is_empty());
}

#[test]
fn test_deblend_disabled_with_high_contrast() {
    let (pixels, labels, data) =
        make_test_component(100, 100, &[(30, 50, 1.0, 2.5), (70, 50, 0.8, 2.5)]);

    let result = deblend_multi_threshold_test(&data, &pixels, &labels, 32, 3, 1.0);

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

    let result = deblend_multi_threshold_test(&data, &pixels, &labels, 32, 3, 0.005);

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

    let result = deblend_multi_threshold_test(&data, &pixels, &labels, 32, 3, 0.1);

    assert!(
        result.len() >= 2,
        "Should find at least the isolated star and close pair"
    );
}

#[test]
fn test_equal_brightness_stars() {
    let (pixels, labels, data) =
        make_test_component(100, 100, &[(30, 50, 1.0, 2.5), (70, 50, 1.0, 2.5)]);

    let result = deblend_multi_threshold_test(&data, &pixels, &labels, 32, 3, 0.005);

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

    let result_pass = deblend_multi_threshold_test(&data, &pixels, &labels, 32, 3, 0.09);

    let result_fail = deblend_multi_threshold_test(&data, &pixels, &labels, 32, 3, 0.15);

    assert!(
        result_pass.len() >= result_fail.len(),
        "Lower contrast threshold should find more or equal objects"
    );
}

#[test]
fn test_pixel_assignment_conservation() {
    let (pixels, labels, data) =
        make_test_component(100, 100, &[(30, 50, 1.0, 2.5), (70, 50, 0.8, 2.5)]);

    let result = deblend_multi_threshold_test(&data, &pixels, &labels, 32, 3, 0.005);

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

    let result = deblend_multi_threshold_test(&data, &pixels, &labels, 32, 3, 0.005);

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

    let result = deblend_multi_threshold_test(&data, &pixels, &labels, 32, 3, 0.005);

    assert_eq!(result.len(), 2, "Diagonally separated stars should deblend");
}

#[test]
fn test_n_thresholds_effect() {
    let (pixels, labels, data) =
        make_test_component(100, 100, &[(35, 50, 1.0, 2.5), (65, 50, 0.9, 2.5)]);
    let result_few = deblend_multi_threshold_test(&data, &pixels, &labels, 4, 3, 0.005);
    let result_many = deblend_multi_threshold_test(&data, &pixels, &labels, 64, 3, 0.005);

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

    let result = deblend_multi_threshold_test(&data, &pixels, &labels, 32, 3, 0.005);

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

    let result = deblend_multi_threshold_test(&data, &pixels, &labels, 32, 3, 0.005);

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

    let result = deblend_multi_threshold_test(&data, &pixels, &labels, 32, 3, 0.005);

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

    let result = deblend_multi_threshold_test(&data, &pixels, &labels, 64, 3, 0.005);

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

    let result = deblend_multi_threshold_test(&data, &pixels, &labels, 128, 2, 0.001);

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

    let result1 = deblend_multi_threshold_test(&data, &pixels, &labels, 32, 3, 0.005);
    let result2 = deblend_multi_threshold_test(&data, &pixels, &labels, 32, 3, 0.005);
    let result3 = deblend_multi_threshold_test(&data, &pixels, &labels, 32, 3, 0.005);

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

    let result = deblend_multi_threshold_test(&data, &pixels, &labels, 32, 3, 0.005);

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

    let result = deblend_multi_threshold_test(&data, &pixels, &labels, 32, 3, 0.005);

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

    let result = deblend_multi_threshold_test(&data, &pixels, &labels, 32, 3, 0.005);

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

    let result = deblend_multi_threshold_test(&data, &pixels, &labels, 1, 3, 0.005);

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

    let result = deblend_multi_threshold_test(&data, &pixels, &labels, 0, 3, 0.005);

    // Should produce single object
    assert_eq!(result.len(), 1, "Should produce one object");
    assert_eq!(result[0].area, data.area);
}

// ========================================================================
// Unit tests for PixelGrid
// ========================================================================

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
    let mut region_pool = Vec::new();

    find_connected_regions_grid(
        &pixels,
        &mut regions,
        &mut region_pool,
        &mut grid,
        &mut queue,
    );

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
fn test_pixel_grid_reuse() {
    let mut grid = PixelGrid::empty();
    let mut queue = Vec::new();
    let mut regions = Vec::new();
    let mut region_pool = Vec::new();

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
    find_connected_regions_grid(
        &pixels1,
        &mut regions,
        &mut region_pool,
        &mut grid,
        &mut queue,
    );
    assert_eq!(regions.len(), 2);

    // Reuse with different pixels — grid state should be properly reset
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
    find_connected_regions_grid(
        &pixels2,
        &mut regions,
        &mut region_pool,
        &mut grid,
        &mut queue,
    );

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
    let mut queue = Vec::new();
    let mut regions = Vec::new();
    let mut region_pool = Vec::new();

    find_connected_regions_grid(
        &pixels,
        &mut regions,
        &mut region_pool,
        &mut grid,
        &mut queue,
    );

    assert_eq!(regions.len(), 1);
    assert_eq!(regions[0].len(), 1);
    assert_eq!(regions[0][0].value, 42.0);
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
    let mut region_pool = Vec::new();
    let mut grid = PixelGrid::empty();
    let mut queue = Vec::new();

    find_connected_regions_grid(
        &pixels,
        &mut regions,
        &mut region_pool,
        &mut grid,
        &mut queue,
    );

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
    let mut region_pool = Vec::new();
    let mut grid = PixelGrid::empty();
    let mut queue = Vec::new();

    find_connected_regions_grid(
        &pixels,
        &mut regions,
        &mut region_pool,
        &mut grid,
        &mut queue,
    );

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
    let mut region_pool = Vec::new();
    let mut grid = PixelGrid::empty();
    let mut queue = Vec::new();

    find_connected_regions_grid(
        &pixels,
        &mut regions,
        &mut region_pool,
        &mut grid,
        &mut queue,
    );

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
    let mut region_pool = Vec::new();
    let mut grid = PixelGrid::empty();
    let mut queue = Vec::new();

    find_connected_regions_grid(
        &pixels,
        &mut regions,
        &mut region_pool,
        &mut grid,
        &mut queue,
    );

    assert_eq!(regions.len(), 1, "All pixels should be in one region");
    assert_eq!(regions[0].len(), 9, "All 9 pixels should be found");
}

#[test]
fn test_pixel_grid_values_generation_isolation() {
    // Verify that generation-counter-based value storage correctly isolates
    // values between successive reset_with_pixels calls. Stale values from
    // a previous population must not be visible after reset.
    let mut grid = PixelGrid::empty();

    // First population: large grid with many pixels
    let pixels1: Vec<Pixel> = (0..100)
        .map(|i| Pixel {
            pos: Vec2us::new(10 + i, 10),
            value: 42.0,
        })
        .collect();
    grid.reset_with_pixels(&pixels1);

    // Second population: small grid with only 2 pixels
    let pixels2 = vec![
        Pixel {
            pos: Vec2us::new(50, 10),
            value: 1.0,
        },
        Pixel {
            pos: Vec2us::new(51, 10),
            value: 2.0,
        },
    ];
    grid.reset_with_pixels(&pixels2);

    // BFS should only find the 2 pixels from the second population,
    // not the stale 100 pixels from the first.
    let mut regions = Vec::new();
    let mut region_pool = Vec::new();
    let mut queue = Vec::new();
    find_connected_regions_grid(
        &pixels2,
        &mut regions,
        &mut region_pool,
        &mut grid,
        &mut queue,
    );

    assert_eq!(regions.len(), 1);
    assert_eq!(
        regions[0].len(),
        2,
        "Should find exactly 2 pixels, not stale values from previous population"
    );
}

#[test]
fn test_pixel_grid_repeated_resets_same_positions() {
    // Verify correctness when the same positions are repopulated with
    // different values across multiple resets.
    let mut grid = PixelGrid::empty();
    let mut regions = Vec::new();
    let mut region_pool = Vec::new();
    let mut queue = Vec::new();

    for round in 0..10 {
        let pixels = vec![
            Pixel {
                pos: Vec2us::new(5, 5),
                value: round as f32,
            },
            Pixel {
                pos: Vec2us::new(6, 5),
                value: round as f32 + 0.5,
            },
        ];

        find_connected_regions_grid(
            &pixels,
            &mut regions,
            &mut region_pool,
            &mut grid,
            &mut queue,
        );

        assert_eq!(regions.len(), 1, "Round {}: should find 1 region", round);
        assert_eq!(
            regions[0].len(),
            2,
            "Round {}: should find exactly 2 pixels",
            round
        );

        // Verify values match current round, not stale from previous
        let mut values: Vec<f32> = regions[0].iter().map(|p| p.value).collect();
        values.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert_eq!(values[0], round as f32, "Round {}: wrong value", round);
        assert_eq!(
            values[1],
            round as f32 + 0.5,
            "Round {}: wrong value",
            round
        );
    }
}

#[test]
fn test_connected_regions_pixels_at_coordinate_zero() {
    // Regression test: pixels at coordinate (0, 0) caused segfault when
    // the grid border was computed with saturating_sub instead of wrapping_sub.
    // The border must always be guaranteed even at the image edge.
    let pixels = vec![
        Pixel {
            pos: Vec2us::new(0, 0),
            value: 1.0,
        },
        Pixel {
            pos: Vec2us::new(1, 0),
            value: 2.0,
        },
        Pixel {
            pos: Vec2us::new(0, 1),
            value: 3.0,
        },
    ];

    let mut regions = Vec::new();
    let mut region_pool = Vec::new();
    let mut grid = PixelGrid::empty();
    let mut queue = Vec::new();

    find_connected_regions_grid(
        &pixels,
        &mut regions,
        &mut region_pool,
        &mut grid,
        &mut queue,
    );

    assert_eq!(regions.len(), 1, "All 3 pixels should form one region");
    assert_eq!(regions[0].len(), 3);

    // Verify absolute coordinates are preserved correctly
    let mut positions: Vec<(usize, usize)> =
        regions[0].iter().map(|p| (p.pos.x, p.pos.y)).collect();
    positions.sort();
    assert_eq!(positions, vec![(0, 0), (0, 1), (1, 0)]);
}

#[test]
fn test_connected_regions_two_groups_near_zero() {
    // Two disconnected groups near coordinate 0
    let pixels = vec![
        Pixel {
            pos: Vec2us::new(0, 0),
            value: 1.0,
        },
        Pixel {
            pos: Vec2us::new(5, 5),
            value: 2.0,
        },
    ];

    let mut regions = Vec::new();
    let mut region_pool = Vec::new();
    let mut grid = PixelGrid::empty();
    let mut queue = Vec::new();

    find_connected_regions_grid(
        &pixels,
        &mut regions,
        &mut region_pool,
        &mut grid,
        &mut queue,
    );

    assert_eq!(regions.len(), 2, "Should find 2 separate regions");
    assert_eq!(regions[0].len(), 1);
    assert_eq!(regions[1].len(), 1);
}

// ========================================================================
// Tests for find_connected_regions_grid_into (ArrayVec capacity limit)
// ========================================================================

#[test]
fn test_connected_regions_grid_into_basic() {
    // Three separate regions, ArrayVec capacity 4 — all should fit
    let pixels = vec![
        Pixel {
            pos: Vec2us::new(0, 0),
            value: 1.0,
        },
        Pixel {
            pos: Vec2us::new(10, 10),
            value: 2.0,
        },
        Pixel {
            pos: Vec2us::new(20, 20),
            value: 3.0,
        },
    ];

    let mut regions: ArrayVec<Vec<Pixel>, 4> = ArrayVec::new();
    let mut region_pool = Vec::new();
    let mut grid = PixelGrid::empty();
    let mut queue = Vec::new();

    find_connected_regions_grid_into(
        &pixels,
        &mut regions,
        &mut region_pool,
        &mut grid,
        &mut queue,
    );

    assert_eq!(regions.len(), 3, "Should find all 3 separate regions");
    for region in &regions {
        assert_eq!(region.len(), 1);
    }
}

#[test]
fn test_connected_regions_grid_into_capacity_limit() {
    // Five separate pixels but ArrayVec capacity of 2 — should stop at 2
    let pixels = vec![
        Pixel {
            pos: Vec2us::new(0, 0),
            value: 1.0,
        },
        Pixel {
            pos: Vec2us::new(10, 0),
            value: 2.0,
        },
        Pixel {
            pos: Vec2us::new(20, 0),
            value: 3.0,
        },
        Pixel {
            pos: Vec2us::new(30, 0),
            value: 4.0,
        },
        Pixel {
            pos: Vec2us::new(40, 0),
            value: 5.0,
        },
    ];

    let mut regions: ArrayVec<Vec<Pixel>, 2> = ArrayVec::new();
    let mut region_pool = Vec::new();
    let mut grid = PixelGrid::empty();
    let mut queue = Vec::new();

    find_connected_regions_grid_into(
        &pixels,
        &mut regions,
        &mut region_pool,
        &mut grid,
        &mut queue,
    );

    assert_eq!(regions.len(), 2, "Should stop at capacity limit");
}

#[test]
fn test_connected_regions_grid_into_recycles_previous() {
    // Verify that calling twice recycles the previous regions to pool
    let pixels = vec![
        Pixel {
            pos: Vec2us::new(5, 5),
            value: 1.0,
        },
        Pixel {
            pos: Vec2us::new(15, 15),
            value: 2.0,
        },
    ];

    let mut regions: ArrayVec<Vec<Pixel>, 4> = ArrayVec::new();
    let mut region_pool = Vec::new();
    let mut grid = PixelGrid::empty();
    let mut queue = Vec::new();

    // First call
    find_connected_regions_grid_into(
        &pixels,
        &mut regions,
        &mut region_pool,
        &mut grid,
        &mut queue,
    );
    assert_eq!(regions.len(), 2);
    assert!(region_pool.is_empty());

    // Second call — previous regions should be recycled to pool then reused
    find_connected_regions_grid_into(
        &pixels,
        &mut regions,
        &mut region_pool,
        &mut grid,
        &mut queue,
    );
    assert_eq!(regions.len(), 2);
    // Pool should be empty because the 2 recycled vecs were reused for the 2 new regions
    assert!(region_pool.is_empty());
}

#[test]
fn test_pixel_grid_generation_wrap_to_zero_guard() {
    // Verify that wrapping generation counter from u32::MAX to 0 is handled
    // correctly — generation 0 is skipped because generation arrays are
    // initialized to 0, so wrapping to 0 would make all cells appear valid.
    let mut grid = PixelGrid::empty();

    // First population to set up grid dimensions
    let pixels_large: Vec<Pixel> = (0..20)
        .map(|i| Pixel {
            pos: Vec2us::new(i, 0),
            value: 99.0,
        })
        .collect();
    grid.reset_with_pixels(&pixels_large);

    // Force generation counter to u32::MAX so next reset wraps
    grid.current_generation = u32::MAX;

    // Small population — reset should wrap past 0 to 1
    let pixels_small = vec![
        Pixel {
            pos: Vec2us::new(5, 0),
            value: 1.0,
        },
        Pixel {
            pos: Vec2us::new(6, 0),
            value: 2.0,
        },
    ];
    grid.reset_with_pixels(&pixels_small);

    assert_ne!(
        grid.current_generation, 0,
        "Generation 0 must be skipped on wrap"
    );

    // BFS should find exactly the 2 new pixels, not stale data
    let mut regions = Vec::new();
    let mut region_pool = Vec::new();
    let mut queue = Vec::new();
    find_connected_regions_grid(
        &pixels_small,
        &mut regions,
        &mut region_pool,
        &mut grid,
        &mut queue,
    );

    assert_eq!(regions.len(), 1);
    assert_eq!(
        regions[0].len(),
        2,
        "Should find exactly 2 pixels after generation wrap, not stale values"
    );
}
