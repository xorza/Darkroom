//! Tests for multi-threshold deblending.

use super::*;

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

    let label_map = LabelMap::from_raw(labels, 1);
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
    let labels = LabelMap::from_raw(labels_buf, 0);
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

    let labels = LabelMap::from_raw(labels_buf, 1);
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

    let labels = LabelMap::from_raw(labels_buf, 1);
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
