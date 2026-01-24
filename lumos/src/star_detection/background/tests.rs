//! Tests for background estimation.

use super::*;

#[test]
fn test_uniform_background() {
    let width = 128;
    let height = 128;
    let pixels: Vec<f32> = vec![0.5; width * height];

    let bg = estimate_background(&pixels, width, height, 32);

    for y in 0..height {
        for x in 0..width {
            let val = bg.get_background(x, y);
            assert!(
                (val - 0.5).abs() < 0.01,
                "Background at ({}, {}) = {}, expected ~0.5",
                x,
                y,
                val
            );
        }
    }
}

#[test]
fn test_gradient_background() {
    let width = 128;
    let height = 128;
    let pixels: Vec<f32> = (0..height)
        .flat_map(|y| (0..width).map(move |x| (x + y) as f32 / 256.0))
        .collect();

    let bg = estimate_background(&pixels, width, height, 32);

    let corner_00 = bg.get_background(0, 0);
    let corner_end = bg.get_background(127, 127);
    assert!(corner_end > corner_00, "Gradient not preserved");
}

#[test]
fn test_background_with_stars() {
    let width = 128;
    let height = 128;
    let mut pixels: Vec<f32> = vec![0.1; width * height];

    // Add bright spots (stars)
    pixels[64 * width + 64] = 1.0;
    pixels[32 * width + 32] = 0.9;
    pixels[96 * width + 96] = 0.95;

    let bg = estimate_background(&pixels, width, height, 32);

    // Median is robust to outliers
    let center_bg = bg.get_background(64, 64);
    assert!(
        (center_bg - 0.1).abs() < 0.05,
        "Background at star = {}, expected ~0.1",
        center_bg
    );
}

#[test]
fn test_noise_estimation() {
    let width = 128;
    let height = 128;
    let pixels: Vec<f32> = vec![0.5; width * height];

    let bg = estimate_background(&pixels, width, height, 32);

    let noise = bg.get_noise(64, 64);
    assert!(noise < 0.01, "Noise = {}, expected near zero", noise);
}

#[test]
fn test_subtract_method() {
    let width = 128;
    let height = 128;
    let pixels: Vec<f32> = vec![0.3; width * height];

    let bg = estimate_background(&pixels, width, height, 32);

    let subtracted = bg.subtract(&pixels, 64, 64);
    assert!(
        subtracted.abs() < 0.01,
        "Subtracted = {}, expected ~0",
        subtracted
    );

    // Test with bright pixel
    let mut bright = pixels.clone();
    bright[64 * width + 64] = 0.8;
    let sub_bright = bg.subtract(&bright, 64, 64);
    assert!(
        (sub_bright - 0.5).abs() < 0.05,
        "Subtracted bright = {}, expected ~0.5",
        sub_bright
    );
}

#[test]
fn test_non_square_image() {
    let width = 256;
    let height = 64;
    let pixels: Vec<f32> = vec![0.4; width * height];

    let bg = estimate_background(&pixels, width, height, 32);

    assert_eq!(bg.width, width);
    assert_eq!(bg.height, height);
    assert!((bg.get_background(0, 0) - 0.4).abs() < 0.01);
    assert!((bg.get_background(255, 63) - 0.4).abs() < 0.01);
}

#[test]
fn test_sigma_clipping_rejects_outliers() {
    let width = 64;
    let height = 64;
    let mut pixels: Vec<f32> = vec![0.2; width * height];

    // 10% bright outliers
    for i in 0..(width * height / 10) {
        pixels[i * 10] = 0.95;
    }

    let bg = estimate_background(&pixels, width, height, 32);

    let bg_val = bg.get_background(32, 32);
    assert!(
        (bg_val - 0.2).abs() < 0.05,
        "Background = {}, expected ~0.2",
        bg_val
    );
}

#[test]
fn test_interpolation_produces_valid_values() {
    // Verify interpolation produces continuous (no NaN/Inf) values
    let width = 128;
    let height = 128;

    let pixels: Vec<f32> = (0..height)
        .flat_map(|y| (0..width).map(move |x| (x + y) as f32 / 256.0))
        .collect();

    let bg = estimate_background(&pixels, width, height, 32);

    for y in 0..height {
        for x in 0..width {
            let val = bg.get_background(x, y);
            assert!(val.is_finite(), "NaN/Inf at ({},{})", x, y);
            assert!(
                (0.0..=1.0).contains(&val),
                "Out of range at ({},{}): {}",
                x,
                y,
                val
            );
        }
    }
}

#[test]
fn test_large_image() {
    let width = 512;
    let height = 512;
    let pixels: Vec<f32> = vec![0.33; width * height];

    let bg = estimate_background(&pixels, width, height, 64);

    assert!((bg.get_background(0, 0) - 0.33).abs() < 0.01);
    assert!((bg.get_background(255, 255) - 0.33).abs() < 0.01);
    assert!((bg.get_background(511, 511) - 0.33).abs() < 0.01);
}

#[test]
fn test_different_tile_sizes() {
    let width = 256;
    let height = 256;
    let pixels: Vec<f32> = vec![0.5; width * height];

    // Test min and max tile sizes
    for tile_size in [16, 32, 64, 128, 256] {
        let bg = estimate_background(&pixels, width, height, tile_size);
        assert!(
            (bg.get_background(128, 128) - 0.5).abs() < 0.01,
            "Failed for tile_size={}",
            tile_size
        );
    }
}

#[test]
#[should_panic(expected = "Tile size must be between 16 and 256")]
fn test_tile_size_too_small() {
    let pixels: Vec<f32> = vec![0.5; 64 * 64];
    estimate_background(&pixels, 64, 64, 8);
}

#[test]
#[should_panic(expected = "Tile size must be between 16 and 256")]
fn test_tile_size_too_large() {
    let pixels: Vec<f32> = vec![0.5; 64 * 64];
    estimate_background(&pixels, 64, 64, 512);
}

#[test]
#[should_panic(expected = "Image must be at least tile_size x tile_size")]
fn test_image_too_small() {
    let pixels: Vec<f32> = vec![0.5; 32 * 32];
    estimate_background(&pixels, 32, 32, 64);
}
