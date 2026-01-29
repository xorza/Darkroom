//! Benchmark module for background estimation.
//! Run with: cargo bench --package lumos --features bench background

use bench::quick_bench;

use crate::common::Buffer2;
use crate::{IterativeBackgroundConfig, estimate_background_iterative};

use std::hint::black_box;

/// Generate a synthetic star field image for benchmarking using deterministic patterns.
fn generate_test_image(width: usize, height: usize) -> Vec<f32> {
    let mut pixels = vec![0.1f32; width * height];

    // Add deterministic "noise" pattern using simple hash
    for (i, p) in pixels.iter_mut().enumerate() {
        // Simple deterministic pseudo-random based on index
        let hash = ((i as u32).wrapping_mul(2654435761)) as f32 / u32::MAX as f32;
        *p += (hash - 0.5) * 0.02; // Small noise: -0.01 to +0.01
    }

    // Add synthetic stars at deterministic positions
    let num_stars = (width * height) / 1000;
    for star_idx in 0..num_stars {
        // Deterministic star positions based on index
        let hash1 = ((star_idx as u32).wrapping_mul(2654435761)) as usize;
        let hash2 = ((star_idx as u32).wrapping_mul(1597334677)) as usize;
        let hash3 = ((star_idx as u32).wrapping_mul(805306457)) as usize;
        let hash4 = ((star_idx as u32).wrapping_mul(402653189)) as usize;

        let cx = 10 + (hash1 % (width - 20));
        let cy = 10 + (hash2 % (height - 20));
        let brightness = 0.3 + (hash3 % 600) as f32 / 1000.0; // 0.3 to 0.9
        let sigma = 1.5 + (hash4 % 150) as f32 / 100.0; // 1.5 to 3.0

        // Add Gaussian star
        for dy in -5i32..=5 {
            for dx in -5i32..=5 {
                let x = (cx as i32 + dx) as usize;
                let y = (cy as i32 + dy) as usize;
                if x < width && y < height {
                    let r2 = (dx * dx + dy * dy) as f32;
                    let value = brightness * (-r2 / (2.0 * sigma * sigma)).exp();
                    pixels[y * width + x] += value;
                }
            }
        }
    }

    pixels
}

#[quick_bench(warmup_iters = 2, iters = 5)]
fn estimate_background_iterative_6k(b: ::bench::Bencher) {
    let width = 6144;
    let height = 6144;

    let pixels = Buffer2::new(width, height, generate_test_image(width, height));
    let iter_config = IterativeBackgroundConfig::default();

    b.bench(|| black_box(estimate_background_iterative(&pixels, 64, &iter_config)));
}
