//! Tests for background estimation.

use super::*;

#[test]
fn test_uniform_background() {
    // Uniform image should have constant background
    let width = 100;
    let height = 100;
    let pixels: Vec<f32> = vec![0.5; width * height];

    let bg = estimate_background(&pixels, width, height, 32);

    // All background values should be close to 0.5
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
    // Linear gradient should be approximated
    let width = 100;
    let height = 100;
    let pixels: Vec<f32> = (0..height)
        .flat_map(|y| (0..width).map(move |x| (x + y) as f32 / 200.0))
        .collect();

    let bg = estimate_background(&pixels, width, height, 32);

    // Background should roughly follow the gradient
    let corner_00 = bg.get_background(0, 0);
    let corner_99 = bg.get_background(99, 99);
    assert!(corner_99 > corner_00, "Gradient not preserved");
}

#[test]
fn test_background_with_stars() {
    // Background with a few bright "stars" - should not affect median much
    let width = 100;
    let height = 100;
    let mut pixels: Vec<f32> = vec![0.1; width * height];

    // Add a few bright spots
    pixels[50 * width + 50] = 1.0;
    pixels[25 * width + 25] = 0.9;
    pixels[75 * width + 75] = 0.95;

    let bg = estimate_background(&pixels, width, height, 32);

    // Background should still be close to 0.1 (median is robust to outliers)
    let center_bg = bg.get_background(50, 50);
    assert!(
        (center_bg - 0.1).abs() < 0.05,
        "Background at star = {}, expected ~0.1",
        center_bg
    );
}

#[test]
fn test_noise_estimation() {
    let width = 64;
    let height = 64;
    let pixels: Vec<f32> = vec![0.5; width * height];

    let bg = estimate_background(&pixels, width, height, 32);

    // Uniform image should have near-zero noise
    let noise = bg.get_noise(32, 32);
    assert!(noise < 0.01, "Noise = {}, expected near zero", noise);
}

#[test]
#[should_panic(expected = "Tile size must be at least 8")]
fn test_tile_size_too_small() {
    let pixels: Vec<f32> = vec![0.5; 100];
    estimate_background(&pixels, 10, 10, 4);
}
