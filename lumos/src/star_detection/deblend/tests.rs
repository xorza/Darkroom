//! Integration tests for deblending algorithms.

use super::*;
use crate::common::Buffer2;

fn make_gaussian_star(cx: usize, cy: usize, amplitude: f32, sigma: f32) -> Vec<Pixel> {
    let mut pixels = Vec::new();
    let radius = (sigma * 4.0).ceil() as i32;

    for dy in -radius..=radius {
        for dx in -radius..=radius {
            let x = (cx as i32 + dx) as usize;
            let y = (cy as i32 + dy) as usize;
            let r2 = (dx * dx + dy * dy) as f32;
            let value = amplitude * (-r2 / (2.0 * sigma * sigma)).exp();
            if value > 0.001 {
                pixels.push(Pixel { x, y, value });
            }
        }
    }

    pixels
}

/// Helper to create image and labels from star pixels.
fn create_image_and_labels(
    width: usize,
    height: usize,
    star_pixels: &[Pixel],
    label: u32,
) -> (Buffer2<f32>, Vec<u32>) {
    let mut pixels = vec![0.0f32; width * height];
    let mut labels = vec![0u32; width * height];

    for p in star_pixels {
        if p.x < width && p.y < height {
            let idx = p.y * width + p.x;
            pixels[idx] = p.value;
            labels[idx] = label;
        }
    }

    (Buffer2::new(width, height, pixels), labels)
}

/// Compute bounding box and area from labels.
fn compute_bbox(labels: &[u32], width: usize, label: u32) -> (usize, usize, usize, usize, usize) {
    let mut x_min = usize::MAX;
    let mut x_max = 0;
    let mut y_min = usize::MAX;
    let mut y_max = 0;
    let mut area = 0;

    for (idx, &l) in labels.iter().enumerate() {
        if l == label {
            let x = idx % width;
            let y = idx / width;
            x_min = x_min.min(x);
            x_max = x_max.max(x);
            y_min = y_min.min(y);
            y_max = y_max.max(y);
            area += 1;
        }
    }

    (x_min, x_max, y_min, y_max, area)
}

#[test]
fn test_local_vs_multi_threshold_single_star() {
    // Both algorithms should produce same result for single star
    let width = 100;
    let height = 100;

    let star = make_gaussian_star(50, 50, 1.0, 3.0);
    let (pixels_buf, labels) = create_image_and_labels(width, height, &star, 1);
    let (x_min, x_max, y_min, y_max, area) = compute_bbox(&labels, width, 1);

    // Local maxima deblending
    let data = ComponentData {
        x_min,
        x_max,
        y_min,
        y_max,
        label: 1,
        area,
    };

    let local_config = DeblendConfig::default();
    let local_result = deblend_local_maxima(&data, &pixels_buf, &labels, &local_config);

    // Multi-threshold deblending
    let mt_config = MultiThresholdDeblendConfig::default();
    let mt_result = deblend_component(&star, width, 0.01, &mt_config);

    assert_eq!(local_result.len(), 1);
    assert_eq!(mt_result.len(), 1);

    // Peak positions should be similar
    assert!((local_result[0].peak_x as i32 - mt_result[0].peak_x as i32).abs() <= 1);
    assert!((local_result[0].peak_y as i32 - mt_result[0].peak_y as i32).abs() <= 1);
}

#[test]
fn test_local_vs_multi_threshold_two_stars() {
    // Both algorithms should find two stars when well-separated
    let width = 100;
    let height = 100;

    let star1 = make_gaussian_star(30, 50, 1.0, 2.5);
    let star2 = make_gaussian_star(70, 50, 0.8, 2.5);

    let mut image = vec![0.0f32; width * height];
    let mut labels = vec![0u32; width * height];

    for p in star1.iter().chain(star2.iter()) {
        if p.x < width && p.y < height {
            let idx = p.y * width + p.x;
            image[idx] += p.value;
            labels[idx] = 1; // All pixels in same component
        }
    }

    // Collect component pixels for multi-threshold (which still uses Vec<Pixel>)
    let component_pixels: Vec<Pixel> = labels
        .iter()
        .enumerate()
        .filter(|&(_, &l)| l == 1)
        .map(|(idx, _)| Pixel {
            x: idx % width,
            y: idx / width,
            value: image[idx],
        })
        .collect();

    let image_buf = Buffer2::new(width, height, image);
    let (x_min, x_max, y_min, y_max, area) = compute_bbox(&labels, width, 1);

    // Local maxima deblending
    let data = ComponentData {
        x_min,
        x_max,
        y_min,
        y_max,
        label: 1,
        area,
    };

    let local_config = DeblendConfig {
        min_separation: 3,
        min_prominence: 0.3,
        ..Default::default()
    };
    let local_result = deblend_local_maxima(&data, &image_buf, &labels, &local_config);

    // Multi-threshold deblending
    let mt_config = MultiThresholdDeblendConfig {
        n_thresholds: 32,
        min_contrast: 0.005,
        min_separation: 3,
    };
    let mt_result = deblend_component(&component_pixels, width, 0.01, &mt_config);

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
