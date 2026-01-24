//! Tests for star detection.

use super::*;
use crate::star_detection::background::estimate_background;

fn make_test_image_with_star(
    width: usize,
    height: usize,
    star_x: usize,
    star_y: usize,
) -> Vec<f32> {
    let mut pixels = vec![0.1f32; width * height];

    // Add a Gaussian-like star
    for dy in -3i32..=3 {
        for dx in -3i32..=3 {
            let x = star_x as i32 + dx;
            let y = star_y as i32 + dy;
            if x >= 0 && x < width as i32 && y >= 0 && y < height as i32 {
                let dist_sq = (dx * dx + dy * dy) as f32;
                let value = 0.1 + 0.8 * (-dist_sq / 4.0).exp();
                pixels[y as usize * width + x as usize] = value;
            }
        }
    }

    pixels
}

#[test]
fn test_detect_single_star() {
    let width = 64;
    let height = 64;
    let pixels = make_test_image_with_star(width, height, 32, 32);

    let bg = estimate_background(&pixels, width, height, 32);
    let config = StarDetectionConfig::default();
    let candidates = detect_stars(&pixels, width, height, &bg, &config);

    assert_eq!(candidates.len(), 1, "Should detect exactly one star");
    let star = &candidates[0];
    assert!(
        star.peak_x >= 30 && star.peak_x <= 34,
        "Peak X should be near 32"
    );
    assert!(
        star.peak_y >= 30 && star.peak_y <= 34,
        "Peak Y should be near 32"
    );
}

#[test]
fn test_detect_multiple_stars() {
    let width = 100;
    let height = 100;
    let mut pixels = vec![0.1f32; width * height];

    // Add three stars
    for (sx, sy) in [(25i32, 25i32), (50, 50), (75, 75)] {
        for dy in -3i32..=3 {
            for dx in -3i32..=3 {
                let x = sx + dx;
                let y = sy + dy;
                if x >= 0 && x < width as i32 && y >= 0 && y < height as i32 {
                    let dist_sq = (dx * dx + dy * dy) as f32;
                    let value = 0.1 + 0.8 * (-dist_sq / 4.0).exp();
                    pixels[y as usize * width + x as usize] = value;
                }
            }
        }
    }

    let bg = estimate_background(&pixels, width, height, 32);
    let config = StarDetectionConfig {
        edge_margin: 5,
        ..Default::default()
    };
    let candidates = detect_stars(&pixels, width, height, &bg, &config);

    assert_eq!(candidates.len(), 3, "Should detect three stars");
}

#[test]
fn test_reject_edge_stars() {
    let width = 64;
    let height = 64;
    // Star at edge (x=5, y=32) should be rejected with edge_margin=10
    let pixels = make_test_image_with_star(width, height, 5, 32);

    let bg = estimate_background(&pixels, width, height, 32);
    let config = StarDetectionConfig {
        edge_margin: 10,
        ..Default::default()
    };
    let candidates = detect_stars(&pixels, width, height, &bg, &config);

    assert!(candidates.is_empty(), "Edge star should be rejected");
}

#[test]
fn test_reject_small_objects() {
    let width = 64;
    let height = 64;
    let mut pixels = vec![0.1f32; width * height];

    // Single bright pixel - after dilation (radius 2), becomes 25 pixels (5x5).
    // Use min_area > 25 to reject single-pixel noise.
    pixels[32 * width + 32] = 0.9;

    let bg = estimate_background(&pixels, width, height, 32);
    let config = StarDetectionConfig {
        min_area: 26, // Must be > 25 to reject dilated single pixel (radius 2 = 5x5 = 25)
        ..Default::default()
    };
    let candidates = detect_stars(&pixels, width, height, &bg, &config);

    assert!(candidates.is_empty(), "Single pixel should be rejected");
}

#[test]
fn test_empty_image() {
    let width = 64;
    let height = 64;
    let pixels = vec![0.1f32; width * height];

    let bg = estimate_background(&pixels, width, height, 32);
    let config = StarDetectionConfig::default();
    let candidates = detect_stars(&pixels, width, height, &bg, &config);

    assert!(candidates.is_empty(), "Uniform image should have no stars");
}
