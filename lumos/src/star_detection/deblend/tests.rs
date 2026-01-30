//! Integration tests for deblending algorithms.

use super::*;
use crate::common::Buffer2;
use crate::math::Vec2us;
use crate::star_detection::config::DeblendConfig;
use crate::star_detection::detection::LabelMap;

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
fn test_local_vs_multi_threshold_single_star() {
    // Both algorithms should produce same result for single star
    let (pixels, labels, data) = make_test_component(100, 100, &[(50, 50, 1.0, 3.0)]);

    // Local maxima deblending
    let local_config = DeblendConfig::default();
    let local_result = deblend_local_maxima(&data, &pixels, &labels, &local_config);

    // Multi-threshold deblending
    let mt_config = DeblendConfig::default();
    let mt_result = deblend_component(&data, &pixels, &labels, &mt_config);

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
    let local_config = DeblendConfig {
        min_separation: 3,
        min_prominence: 0.3,
        ..Default::default()
    };
    let local_result = deblend_local_maxima(&data, &pixels, &labels, &local_config);

    // Multi-threshold deblending
    let mt_config = DeblendConfig {
        n_thresholds: 32,
        min_contrast: 0.005,
        min_separation: 3,
        ..Default::default()
    };
    let mt_result = deblend_component(&data, &pixels, &labels, &mt_config);

    assert_eq!(local_result.len(), 2, "Local maxima should find 2 stars");
    assert_eq!(mt_result.len(), 2, "Multi-threshold should find 2 stars");
}

#[test]
fn test_deblend_config_conversion() {
    let config = DeblendConfig {
        min_separation: 5,
        min_prominence: 0.5,
        multi_threshold: true,
        n_thresholds: 64,
        min_contrast: 0.01,
    };

    assert_eq!(config.min_separation, 5);
    assert!((config.min_prominence - 0.5).abs() < 1e-6);
    assert!(config.multi_threshold);
    assert_eq!(config.n_thresholds, 64);
    assert!((config.min_contrast - 0.01).abs() < 1e-6);
}
