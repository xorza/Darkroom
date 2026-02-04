//! Tests for local maxima deblending.

use super::*;
use crate::math::Vec2us;
use crate::star_detection::labeling::{LabelMap, label_map_from_raw};

use super::is_local_maximum;

// Default deblend parameters for tests
const DEFAULT_MIN_SEPARATION: usize = 3;
const DEFAULT_MIN_PROMINENCE: f32 = 0.3;

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
fn test_find_single_peak() {
    let (pixels, labels, data) = make_test_component(100, 100, &[(50, 50, 1.0, 3.0)]);

    let peaks = find_local_maxima(
        &data,
        &pixels,
        &labels,
        DEFAULT_MIN_SEPARATION,
        DEFAULT_MIN_PROMINENCE,
    );

    assert_eq!(peaks.len(), 1, "Should find exactly one peak");
    assert!(
        (peaks[0].pos.x as i32 - 50).abs() <= 1,
        "Peak should be near center"
    );
}

#[test]
fn test_find_two_peaks() {
    let (pixels, labels, data) =
        make_test_component(100, 100, &[(30, 50, 1.0, 2.5), (70, 50, 0.8, 2.5)]);

    let peaks = find_local_maxima(&data, &pixels, &labels, 3, 0.3);

    assert_eq!(peaks.len(), 2, "Should find two peaks");
}

#[test]
fn test_deblend_creates_separate_candidates() {
    let (pixels, labels, data) =
        make_test_component(100, 100, &[(30, 50, 1.0, 2.5), (70, 50, 0.8, 2.5)]);

    let candidates = deblend_local_maxima(&data, &pixels, &labels, 3, 0.3);

    assert_eq!(candidates.len(), 2, "Should create two candidates");
    assert!(candidates[0].area > 0);
    assert!(candidates[1].area > 0);
}

#[test]
fn test_iter_pixels_count() {
    let (pixels, labels, data) = make_test_component(100, 100, &[(50, 50, 1.0, 3.0)]);

    let iter_count = data.iter_pixels(&pixels, &labels).count();
    assert_eq!(
        iter_count, data.area,
        "iter_pixels should yield exactly area pixels"
    );
}

#[test]
fn test_euclidean_separation() {
    // Two peaks at distance sqrt(18) ≈ 4.24 apart (diagonal)
    // With min_separation=5, they should be merged (5^2=25 > 18)
    // With min_separation=4, they should be separate (4^2=16 < 18)
    let (pixels, labels, data) =
        make_test_component(100, 100, &[(50, 50, 1.0, 1.5), (53, 53, 0.9, 1.5)]);

    let peaks_merge = find_local_maxima(&data, &pixels, &labels, 5, 0.3);
    assert_eq!(peaks_merge.len(), 1, "Close peaks should merge");

    let peaks_separate = find_local_maxima(&data, &pixels, &labels, 4, 0.3);
    assert_eq!(peaks_separate.len(), 2, "Distant peaks should separate");
}

#[test]
fn test_prominence_filter() {
    // Bright primary peak and dim secondary that should be filtered
    let (pixels, labels, data) =
        make_test_component(100, 100, &[(30, 50, 1.0, 2.5), (70, 50, 0.2, 2.5)]);

    // With high prominence threshold, only bright peak survives
    let peaks = find_local_maxima(&data, &pixels, &labels, 3, 0.5);
    assert_eq!(peaks.len(), 1, "Dim peak should be filtered by prominence");

    // With low prominence threshold, both peaks survive
    let peaks = find_local_maxima(&data, &pixels, &labels, 3, 0.1);
    assert_eq!(peaks.len(), 2, "Both peaks should pass low prominence");
}

#[test]
fn test_deblend_empty_peaks() {
    let (pixels, labels, data) = make_test_component(100, 100, &[(50, 50, 1.0, 3.0)]);
    let empty_peaks: &[Pixel] = &[];

    let candidates = deblend_by_nearest_peak(&data, &pixels, &labels, empty_peaks);
    assert!(
        candidates.is_empty(),
        "Empty peaks should return empty result"
    );
}

#[test]
fn test_deblend_single_peak_returns_full_component() {
    let (pixels, labels, data) = make_test_component(100, 100, &[(50, 50, 1.0, 3.0)]);

    let candidates = deblend_local_maxima(
        &data,
        &pixels,
        &labels,
        DEFAULT_MIN_SEPARATION,
        DEFAULT_MIN_PROMINENCE,
    );

    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].area, data.area);
    assert_eq!(candidates[0].bbox.min.x, data.bbox.min.x);
    assert_eq!(candidates[0].bbox.max.x, data.bbox.max.x);
}

#[test]
fn test_peaks_sorted_by_brightness() {
    let (pixels, labels, data) = make_test_component(
        100,
        100,
        &[(30, 50, 0.5, 2.5), (50, 50, 1.0, 2.5), (70, 50, 0.7, 2.5)],
    );

    let peaks = find_local_maxima(&data, &pixels, &labels, 3, 0.3);

    assert_eq!(peaks.len(), 3);
    assert!(
        peaks[0].value >= peaks[1].value && peaks[1].value >= peaks[2].value,
        "Peaks should be sorted by brightness descending"
    );
}

#[test]
fn test_find_peak_returns_global_max() {
    let (pixels, labels, data) = make_test_component(
        100,
        100,
        &[(30, 50, 0.5, 2.5), (50, 50, 1.0, 2.5), (70, 50, 0.7, 2.5)],
    );

    let peak = data.find_peak(&pixels, &labels);
    assert!(
        (peak.pos.x as i32 - 50).abs() <= 1 && (peak.pos.y as i32 - 50).abs() <= 1,
        "find_peak should return the brightest star's position"
    );
    assert!(peak.value > 0.9, "Peak value should be close to 1.0");
}

#[test]
fn test_deblend_area_conservation() {
    // Total area of deblended candidates should equal original component area
    let (pixels, labels, data) =
        make_test_component(100, 100, &[(30, 50, 1.0, 2.5), (70, 50, 0.8, 2.5)]);

    let candidates = deblend_local_maxima(&data, &pixels, &labels, 3, 0.3);

    let total_area: usize = candidates.iter().map(|c| c.area).sum();
    assert_eq!(
        total_area, data.area,
        "Deblending should conserve total area"
    );
}

#[test]
fn test_peak_replacement_when_brighter() {
    // Two very close peaks - brighter one should replace dimmer
    let (pixels, labels, data) =
        make_test_component(100, 100, &[(50, 50, 1.0, 1.5), (51, 50, 0.8, 1.5)]);

    let peaks = find_local_maxima(&data, &pixels, &labels, 5, 0.3);

    assert_eq!(peaks.len(), 1, "Should merge to single peak");
    assert!(
        peaks[0].value > 0.9,
        "Merged peak should be the brighter one"
    );
}

#[test]
fn test_is_local_maximum_edge_cases() {
    // Test local maximum detection at image boundaries
    let mut pixels = Buffer2::new_filled(10, 10, 0.0f32);

    // Corner pixel (0,0) as local max
    pixels[(0, 0)] = 1.0;
    pixels[(1, 0)] = 0.5;
    pixels[(0, 1)] = 0.5;
    pixels[(1, 1)] = 0.5;

    let corner_pixel = Pixel {
        pos: Vec2us::new(0, 0),
        value: 1.0,
    };
    assert!(
        is_local_maximum(corner_pixel, &pixels),
        "Corner pixel should be local max"
    );

    // Edge pixel
    pixels[(5, 0)] = 1.0;
    pixels[(4, 0)] = 0.5;
    pixels[(6, 0)] = 0.5;
    pixels[(4, 1)] = 0.5;
    pixels[(5, 1)] = 0.5;
    pixels[(6, 1)] = 0.5;

    let edge_pixel = Pixel {
        pos: Vec2us::new(5, 0),
        value: 1.0,
    };
    assert!(
        is_local_maximum(edge_pixel, &pixels),
        "Edge pixel should be local max"
    );
}

#[test]
fn test_is_local_maximum_not_max() {
    let mut pixels = Buffer2::new_filled(10, 10, 0.0f32);

    // Center pixel with brighter neighbor
    pixels[(5, 5)] = 0.5;
    pixels[(6, 5)] = 1.0; // Brighter neighbor

    let pixel = Pixel {
        pos: Vec2us::new(5, 5),
        value: 0.5,
    };
    assert!(
        !is_local_maximum(pixel, &pixels),
        "Pixel with brighter neighbor is not local max"
    );
}

#[test]
fn test_voronoi_partitioning() {
    // Create component with two well-separated peaks
    let (pixels, labels, data) =
        make_test_component(100, 100, &[(25, 50, 1.0, 3.0), (75, 50, 1.0, 3.0)]);

    let peaks = vec![
        Pixel {
            pos: Vec2us::new(25, 50),
            value: 1.0,
        },
        Pixel {
            pos: Vec2us::new(75, 50),
            value: 1.0,
        },
    ];

    let candidates = deblend_by_nearest_peak(&data, &pixels, &labels, &peaks);

    assert_eq!(candidates.len(), 2);
    // Each candidate should have its peak inside its bounding box
    for candidate in &candidates {
        assert!(
            candidate.peak.x >= candidate.bbox.min.x && candidate.peak.x <= candidate.bbox.max.x
        );
        assert!(
            candidate.peak.y >= candidate.bbox.min.y && candidate.peak.y <= candidate.bbox.max.y
        );
    }
}

#[test]
fn test_many_peaks_limited_to_max() {
    // Create component with more peaks than MAX_PEAKS
    let stars: Vec<_> = (0..12)
        .map(|i| (10 + i * 8, 50usize, 1.0 - i as f32 * 0.05, 1.5f32))
        .collect();

    let (pixels, labels, data) = make_test_component(120, 100, &stars);

    let peaks = find_local_maxima(&data, &pixels, &labels, 2, 0.1);

    assert!(
        peaks.len() <= MAX_PEAKS,
        "Should not exceed MAX_PEAKS ({}), got {}",
        MAX_PEAKS,
        peaks.len()
    );
}

#[test]
fn test_plateau_no_local_max() {
    // Flat plateau should have no local maximum (strict inequality)
    let mut pixels = Buffer2::new_filled(10, 10, 0.0f32);
    let mut labels_buf = Buffer2::new_filled(10, 10, 0u32);

    // Create a 3x3 plateau of equal values
    for y in 3..6 {
        for x in 3..6 {
            pixels[(x, y)] = 1.0;
            labels_buf[(x, y)] = 1;
        }
    }

    let labels = label_map_from_raw(labels_buf, 1);
    let data = ComponentData {
        bbox: Aabb::new(Vec2us::new(3, 3), Vec2us::new(5, 5)),
        label: 1,
        area: 9,
    };

    let peaks = find_local_maxima(&data, &pixels, &labels, 1, 0.1);

    // No pixel is strictly greater than all neighbors on a plateau
    assert_eq!(peaks.len(), 0, "Plateau should have no local maxima");
}

#[test]
fn test_single_pixel_is_local_max() {
    // A single isolated pixel is always a local maximum
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

    let peaks = find_local_maxima(&data, &pixels, &labels, 1, 0.1);

    assert_eq!(peaks.len(), 1, "Single pixel should be local max");
    assert_eq!(peaks[0].pos, Vec2us::new(5, 5));
}

#[test]
fn test_equal_brightness_tie_breaking() {
    // Two stars with exactly equal brightness - both should be found
    let (pixels, labels, data) =
        make_test_component(100, 100, &[(30, 50, 1.0, 2.5), (70, 50, 1.0, 2.5)]);

    let peaks = find_local_maxima(&data, &pixels, &labels, 3, 0.3);

    assert_eq!(
        peaks.len(),
        2,
        "Both equal-brightness peaks should be found"
    );
}

#[test]
fn test_voronoi_midpoint_assignment() {
    // Pixel exactly between two peaks goes to first peak (deterministic)
    let mut pixels = Buffer2::new_filled(100, 100, 0.0f32);
    let mut labels_buf = Buffer2::new_filled(100, 100, 0u32);

    // Create a horizontal line of pixels
    for x in 20..80 {
        pixels[(x, 50)] = 0.5;
        labels_buf[(x, 50)] = 1;
    }
    // Two peaks at ends
    pixels[(20, 50)] = 1.0;
    pixels[(79, 50)] = 1.0;

    let labels = label_map_from_raw(labels_buf, 1);
    let data = ComponentData {
        bbox: Aabb::new(Vec2us::new(20, 50), Vec2us::new(79, 50)),
        label: 1,
        area: 60,
    };

    let peaks = vec![
        Pixel {
            pos: Vec2us::new(20, 50),
            value: 1.0,
        },
        Pixel {
            pos: Vec2us::new(79, 50),
            value: 1.0,
        },
    ];

    let candidates = deblend_by_nearest_peak(&data, &pixels, &labels, &peaks);

    assert_eq!(candidates.len(), 2);
    // Total area should be conserved
    let total_area: usize = candidates.iter().map(|c| c.area).sum();
    assert_eq!(total_area, 60);
    // Areas should be roughly equal (midpoint goes to one side)
    assert!(candidates[0].area >= 29 && candidates[0].area <= 31);
    assert!(candidates[1].area >= 29 && candidates[1].area <= 31);
}

#[test]
fn test_diagonal_neighbors() {
    // Peak with diagonal neighbors only
    let mut pixels = Buffer2::new_filled(10, 10, 0.0f32);
    let mut labels_buf = Buffer2::new_filled(10, 10, 0u32);

    // Center peak
    pixels[(5, 5)] = 1.0;
    labels_buf[(5, 5)] = 1;
    // Diagonal neighbors only
    pixels[(4, 4)] = 0.5;
    labels_buf[(4, 4)] = 1;
    pixels[(6, 6)] = 0.5;
    labels_buf[(6, 6)] = 1;
    pixels[(4, 6)] = 0.5;
    labels_buf[(4, 6)] = 1;
    pixels[(6, 4)] = 0.5;
    labels_buf[(6, 4)] = 1;

    let _labels = label_map_from_raw(labels_buf, 1);

    let pixel = Pixel {
        pos: Vec2us::new(5, 5),
        value: 1.0,
    };
    assert!(
        is_local_maximum(pixel, &pixels),
        "Center should be local max with diagonal neighbors"
    );
}

#[test]
fn test_all_corners_local_max() {
    // Test all four corners can be local maxima
    let mut pixels = Buffer2::new_filled(5, 5, 0.0f32);

    // Set corners as peaks
    pixels[(0, 0)] = 1.0;
    pixels[(4, 0)] = 1.0;
    pixels[(0, 4)] = 1.0;
    pixels[(4, 4)] = 1.0;

    let corners = [
        Pixel {
            pos: Vec2us::new(0, 0),
            value: 1.0,
        },
        Pixel {
            pos: Vec2us::new(4, 0),
            value: 1.0,
        },
        Pixel {
            pos: Vec2us::new(0, 4),
            value: 1.0,
        },
        Pixel {
            pos: Vec2us::new(4, 4),
            value: 1.0,
        },
    ];

    for corner in &corners {
        assert!(
            is_local_maximum(*corner, &pixels),
            "Corner {:?} should be local max",
            corner.pos
        );
    }
}

#[test]
fn test_zero_min_separation() {
    // With min_separation=0, separation check always passes (dist² >= 0)
    // Create two peaks that are well-separated (distinct local maxima)
    let (pixels, labels, data) =
        make_test_component(100, 100, &[(30, 50, 1.0, 2.0), (70, 50, 0.9, 2.0)]);

    let peaks = find_local_maxima(&data, &pixels, &labels, 0, 0.1);

    // With zero separation, no merging should occur - both peaks found
    assert_eq!(peaks.len(), 2, "Zero separation should allow all peaks");
}

#[test]
fn test_bbox_contains_peak() {
    // Each deblended candidate's bbox should contain its peak
    let (pixels, labels, data) = make_test_component(
        100,
        100,
        &[(25, 25, 1.0, 2.5), (75, 25, 0.9, 2.5), (50, 75, 0.8, 2.5)],
    );

    let candidates = deblend_local_maxima(&data, &pixels, &labels, 3, 0.3);

    for candidate in &candidates {
        assert!(
            candidate.bbox.contains(candidate.peak),
            "Candidate bbox {:?} should contain peak {:?}",
            candidate.bbox,
            candidate.peak
        );
    }
}

#[test]
fn test_peak_value_matches_pixel() {
    // Candidate's peak_value should match the actual pixel value
    let (pixels, labels, data) = make_test_component(100, 100, &[(50, 50, 1.0, 2.5)]);

    let candidates = deblend_local_maxima(
        &data,
        &pixels,
        &labels,
        DEFAULT_MIN_SEPARATION,
        DEFAULT_MIN_PROMINENCE,
    );

    assert_eq!(candidates.len(), 1);
    let candidate = &candidates[0];
    let actual_value = pixels[(candidate.peak.x, candidate.peak.y)];
    assert!(
        (candidate.peak_value - actual_value).abs() < 1e-6,
        "peak_value {} should match pixel value {}",
        candidate.peak_value,
        actual_value
    );
}
