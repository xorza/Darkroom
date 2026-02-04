//! Benchmarks for connected component labeling.

use crate::common::{BitBuffer2, Buffer2};
use crate::star_detection::config::Config;
use crate::star_detection::config::Connectivity;
use crate::star_detection::mask_dilation::dilate_mask;
use crate::star_detection::threshold_mask::create_threshold_mask;
use crate::testing::synthetic::stamps::benchmark_star_field;
use ::bench::quick_bench;
use std::hint::black_box;

/// Create a threshold mask using the real detection pipeline.
/// Uses background estimation, sigma thresholding, and dilation.
fn create_detection_mask(pixels: &Buffer2<f32>, sigma_threshold: f32) -> BitBuffer2 {
    let width = pixels.width();
    let height = pixels.height();

    // Create background map (same as real pipeline)
    let background = crate::testing::estimate_background(pixels, &Config::default());

    // Create threshold mask
    let mut mask = BitBuffer2::new_filled(width, height, false);
    create_threshold_mask(
        pixels,
        &background.background,
        &background.noise,
        sigma_threshold,
        &mut mask,
    );

    // Dilate mask (same as real pipeline - radius 1)
    let mut dilated = BitBuffer2::new_filled(width, height, false);
    dilate_mask(&mask, 1, &mut dilated);

    dilated
}

#[quick_bench(warmup_iters = 2, iters = 5)]
fn bench_label_map_from_buffer_1k(b: ::bench::Bencher) {
    use super::label_mask_parallel;

    let pixels = benchmark_star_field(1024, 1024, 500, 0.1, 0.01, 42);
    let mask = create_detection_mask(&pixels, 4.0);
    let mut labels = Buffer2::new_filled(1024, 1024, 0u32);

    b.bench(|| {
        labels.pixels_mut().fill(0);
        black_box(label_mask_parallel(
            black_box(&mask),
            &mut labels,
            Connectivity::Four,
        ))
    });
}

#[quick_bench(warmup_iters = 1, iters = 3)]
fn bench_label_map_from_buffer_4k(b: ::bench::Bencher) {
    use super::label_mask_parallel;

    let pixels = benchmark_star_field(4096, 4096, 2000, 0.1, 0.01, 42);
    let mask = create_detection_mask(&pixels, 4.0);
    let mut labels = Buffer2::new_filled(4096, 4096, 0u32);

    b.bench(|| {
        labels.pixels_mut().fill(0);
        black_box(label_mask_parallel(
            black_box(&mask),
            &mut labels,
            Connectivity::Four,
        ))
    });
}

#[quick_bench(warmup_iters = 1, iters = 10)]
fn bench_label_map_from_buffer_6k_globular(b: ::bench::Bencher) {
    use super::label_mask_parallel;

    let pixels = benchmark_star_field(4096, 4096, 50000, 0.1, 0.01, 42);
    let mask = create_detection_mask(&pixels, 4.0);
    let mut labels = Buffer2::new_filled(4096, 4096, 0u32);

    b.bench(|| {
        labels.pixels_mut().fill(0);
        black_box(label_mask_parallel(
            black_box(&mask),
            &mut labels,
            Connectivity::Four,
        ))
    });
}

/// Benchmark to find optimal sequential/parallel threshold.
/// Tests various image sizes around the current threshold (100k pixels).
#[test]
#[ignore]
fn bench_threshold_sweep() {
    use super::{label_mask_parallel, label_mask_sequential};
    use crate::common::Buffer2;
    use crate::star_detection::config::Connectivity;
    use std::time::Instant;

    println!("\n=== Sequential vs Parallel Threshold Benchmark ===\n");
    println!(
        "{:>10} {:>10} {:>12} {:>12} {:>10}",
        "Pixels", "Size", "Sequential", "Parallel", "Winner"
    );
    println!("{}", "-".repeat(60));

    // Test sizes from 10k to 500k pixels
    let test_sizes: Vec<(usize, usize)> = vec![
        (100, 100),   // 10k
        (150, 150),   // 22.5k
        (200, 200),   // 40k
        (250, 250),   // 62.5k
        (300, 300),   // 90k
        (316, 316),   // ~100k (current threshold)
        (350, 350),   // 122.5k
        (400, 400),   // 160k
        (450, 450),   // 202.5k
        (500, 500),   // 250k
        (600, 600),   // 360k
        (700, 700),   // 490k
        (800, 800),   // 640k
        (1000, 1000), // 1M
    ];

    for (width, height) in test_sizes {
        let pixels = benchmark_star_field(width, height, width * height / 200, 0.1, 0.01, 42);
        let mask = create_detection_mask(&pixels, 4.0);
        let total_pixels = width * height;

        // Warmup
        for _ in 0..2 {
            let mut labels = Buffer2::new_filled(width, height, 0u32);
            label_mask_sequential(&mask, &mut labels, Connectivity::Four);
            let mut labels = Buffer2::new_filled(width, height, 0u32);
            label_mask_parallel(&mask, &mut labels, Connectivity::Four);
        }

        // Benchmark sequential
        let iterations = if total_pixels < 100_000 { 20 } else { 10 };
        let start = Instant::now();
        for _ in 0..iterations {
            let mut labels = Buffer2::new_filled(width, height, 0u32);
            black_box(label_mask_sequential(
                black_box(&mask),
                &mut labels,
                Connectivity::Four,
            ));
        }
        let seq_time = start.elapsed() / iterations;

        // Benchmark parallel
        let start = Instant::now();
        for _ in 0..iterations {
            let mut labels = Buffer2::new_filled(width, height, 0u32);
            black_box(label_mask_parallel(
                black_box(&mask),
                &mut labels,
                Connectivity::Four,
            ));
        }
        let par_time = start.elapsed() / iterations;

        let winner = if seq_time <= par_time { "seq" } else { "par" };
        let speedup = if seq_time <= par_time {
            par_time.as_nanos() as f64 / seq_time.as_nanos() as f64
        } else {
            seq_time.as_nanos() as f64 / par_time.as_nanos() as f64
        };

        println!(
            "{:>10} {:>4}x{:<4} {:>10.1}µs {:>10.1}µs {:>6} ({:.2}x)",
            total_pixels,
            width,
            height,
            seq_time.as_nanos() as f64 / 1000.0,
            par_time.as_nanos() as f64 / 1000.0,
            winner,
            speedup
        );
    }

    println!("\nNote: Current threshold is 65,000 pixels");
}
