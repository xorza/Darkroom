//! Simple profiling binary for star detection.
//!
//! Run with:
//!   cargo build --release -p lumos --example profile_star_detection
//!   cargo flamegraph --example profile_star_detection -p lumos -- -F 4999
//!
//! Or with samply:
//!   samply record -r 4999 ./target/release/examples/profile_star_detection

use lumos::{AstroImage, ImageDimensions, StarDetector};

fn generate_synthetic_image(width: usize, height: usize, num_stars: usize) -> AstroImage {
    let background = 0.1f32;
    let mut pixels = vec![background; width * height];

    // Add deterministic noise
    for (i, p) in pixels.iter_mut().enumerate() {
        let hash = ((i as u32).wrapping_mul(2654435761)) as f32 / u32::MAX as f32;
        *p += (hash - 0.5) * 0.02;
    }

    // Add synthetic stars
    for star_idx in 0..num_stars {
        let hash1 = ((star_idx as u32).wrapping_mul(2654435761)) as usize;
        let hash2 = ((star_idx as u32).wrapping_mul(1597334677)) as usize;
        let hash3 = ((star_idx as u32).wrapping_mul(805306457)) as usize;
        let hash4 = ((star_idx as u32).wrapping_mul(402653189)) as usize;

        let cx = 20 + (hash1 % (width - 40));
        let cy = 20 + (hash2 % (height - 40));
        let brightness = 0.3 + (hash3 % 700) as f32 / 1000.0;
        let sigma = 1.5 + (hash4 % 150) as f32 / 100.0;

        for dy in -10i32..=10 {
            for dx in -10i32..=10 {
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

    for p in &mut pixels {
        *p = p.clamp(0.0, 1.0);
    }

    AstroImage::from_pixels(ImageDimensions::new(width, height, 1), pixels)
}

fn main() {
    let width = 4096;
    let height = 4096;
    let num_stars = 3000;
    let iterations = 50;

    eprintln!(
        "Generating {}x{} image with {} stars...",
        width, height, num_stars
    );
    let image = generate_synthetic_image(width, height, num_stars);

    let mut detector = StarDetector::new();

    eprintln!("Running {} iterations (attach profiler now)...", iterations);

    // Small delay to allow attaching profiler
    std::thread::sleep(std::time::Duration::from_secs(1));

    for i in 0..iterations {
        let stars = detector.detect(&image);
        // Prevent optimization
        std::hint::black_box(&stars);
        if (i + 1) % 10 == 0 {
            eprintln!(
                "  Completed {}/{} iterations, found {} stars",
                i + 1,
                iterations,
                stars.stars.len()
            );
        }
    }

    eprintln!("Done.");
}
