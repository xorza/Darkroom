//! Integration tests for deblending algorithms.

use super::*;
use std::collections::HashMap;

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

/// Convert Pixel slice to tuples for multi_threshold compatibility
fn pixels_to_tuples(pixels: &[Pixel]) -> Vec<(usize, usize, f32)> {
    pixels.iter().map(|p| (p.x, p.y, p.value)).collect()
}

#[test]
fn test_local_vs_multi_threshold_single_star() {
    // Both algorithms should produce same result for single star
    let width = 100;
    let height = 100;

    let star = make_gaussian_star(50, 50, 1.0, 3.0);

    let mut pixels = vec![0.0f32; width * height];
    for p in &star {
        if p.x < width && p.y < height {
            pixels[p.y * width + p.x] = p.value;
        }
    }

    // Local maxima deblending
    let data = ComponentData {
        x_min: star.iter().map(|p| p.x).min().unwrap(),
        x_max: star.iter().map(|p| p.x).max().unwrap(),
        y_min: star.iter().map(|p| p.y).min().unwrap(),
        y_max: star.iter().map(|p| p.y).max().unwrap(),
        pixels: star.clone(),
    };

    let local_config = DeblendConfig::default();
    let local_result = deblend_local_maxima(&data, &pixels, width, &local_config);

    // Multi-threshold deblending (needs tuple format)
    let star_tuples = pixels_to_tuples(&star);
    let mt_config = MultiThresholdDeblendConfig::default();
    let mt_result = deblend_component(&pixels, &star_tuples, width, 0.01, &mt_config);

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

    let mut all_pixels: Vec<Pixel> = Vec::new();
    let mut image = vec![0.0f32; width * height];

    for p in star1.iter().chain(star2.iter()) {
        if p.x < width && p.y < height {
            image[p.y * width + p.x] += p.value;
            all_pixels.push(Pixel {
                x: p.x,
                y: p.y,
                value: image[p.y * width + p.x],
            });
        }
    }

    // Deduplicate
    let mut seen: HashMap<(usize, usize), f32> = HashMap::new();
    for p in all_pixels {
        seen.insert((p.x, p.y), p.value);
    }
    let component_pixels: Vec<Pixel> = seen
        .into_iter()
        .map(|((x, y), value)| Pixel { x, y, value })
        .collect();

    // Local maxima deblending
    let data = ComponentData {
        x_min: component_pixels.iter().map(|p| p.x).min().unwrap(),
        x_max: component_pixels.iter().map(|p| p.x).max().unwrap(),
        y_min: component_pixels.iter().map(|p| p.y).min().unwrap(),
        y_max: component_pixels.iter().map(|p| p.y).max().unwrap(),
        pixels: component_pixels.clone(),
    };

    let local_config = DeblendConfig {
        min_separation: 3,
        min_prominence: 0.3,
        ..Default::default()
    };
    let local_result = deblend_local_maxima(&data, &image, width, &local_config);

    // Multi-threshold deblending (needs tuple format)
    let component_tuples = pixels_to_tuples(&component_pixels);
    let mt_config = MultiThresholdDeblendConfig {
        n_thresholds: 32,
        min_contrast: 0.005,
        min_separation: 3,
    };
    let mt_result = deblend_component(&image, &component_tuples, width, 0.01, &mt_config);

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
