//! Integration tests for deblending algorithms.
//! These tests compare behavior between local_maxima and multi_threshold.

use super::*;
use crate::common::Buffer2;
use crate::math::Vec2us;
use crate::star_detection::labeling::LabelMap;
use crate::star_detection::labeling::test_utils::label_map_from_raw;

/// Convenience wrapper for tests — creates fresh buffers per call.
fn deblend_multi_threshold_test(
    data: &ComponentData,
    pixels: &Buffer2<f32>,
    labels: &LabelMap,
    n_thresholds: usize,
    min_separation: usize,
    min_contrast: f32,
) -> smallvec::SmallVec<[Region; MAX_PEAKS]> {
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
fn test_local_vs_multi_threshold_single_star() {
    // Both algorithms should produce same result for single star
    let (pixels, labels, data) = make_test_component(100, 100, &[(50, 50, 1.0, 3.0)]);

    // Local maxima deblending (default: min_separation=3, min_prominence=0.3)
    let local_result = deblend_local_maxima(&data, &pixels, &labels, 3, 0.3);

    // Multi-threshold deblending (default: n_thresholds=32, min_separation=3, min_contrast=0.005)
    let mt_result = deblend_multi_threshold_test(&data, &pixels, &labels, 32, 3, 0.005);

    assert_eq!(local_result.len(), 1);
    assert_eq!(mt_result.len(), 1);

    // Peak positions should be similar
    assert!((local_result[0].peak.x as i32 - mt_result[0].peak.x as i32).abs() <= 1);
    assert!((local_result[0].peak.y as i32 - mt_result[0].peak.y as i32).abs() <= 1);
}

#[test]
fn test_local_vs_multi_threshold_two_stars() {
    // Both algorithms should find two stars when well-separated
    let (pixels, labels, data) =
        make_test_component(100, 100, &[(30, 50, 1.0, 2.5), (70, 50, 0.8, 2.5)]);

    // Local maxima deblending
    let local_result = deblend_local_maxima(&data, &pixels, &labels, 3, 0.3);

    // Multi-threshold deblending
    let mt_result = deblend_multi_threshold_test(&data, &pixels, &labels, 32, 3, 0.005);

    assert_eq!(local_result.len(), 2, "Local maxima should find 2 stars");
    assert_eq!(mt_result.len(), 2, "Multi-threshold should find 2 stars");
}
