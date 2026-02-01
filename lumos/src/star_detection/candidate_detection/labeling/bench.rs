//! Benchmarks for connected component labeling.

use super::LabelMap;
use crate::common::BitBuffer2;
use crate::star_detection::background::{BackgroundConfig, BackgroundMap};
use crate::star_detection::mask_dilation::dilate_mask;
use crate::star_detection::threshold_mask::create_threshold_mask;
use crate::testing::synthetic::stamps::benchmark_star_field;
use ::bench::quick_bench;
use std::hint::black_box;

/// Create a threshold mask using the real detection pipeline.
/// Uses background estimation, sigma thresholding, and dilation.
fn create_detection_mask(pixels: &crate::common::Buffer2<f32>, sigma_threshold: f32) -> BitBuffer2 {
    let width = pixels.width();
    let height = pixels.height();

    // Create background map (same as real pipeline)
    let background = BackgroundMap::new(pixels, &BackgroundConfig::default());

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
fn bench_label_map_from_mask_1k(b: ::bench::Bencher) {
    let pixels = benchmark_star_field(1024, 1024, 500, 0.1, 0.01, 42);
    let mask = create_detection_mask(&pixels, 4.0);

    b.bench(|| black_box(LabelMap::from_mask(black_box(&mask))));
}

#[quick_bench(warmup_iters = 1, iters = 3)]
fn bench_label_map_from_mask_4k(b: ::bench::Bencher) {
    let pixels = benchmark_star_field(4096, 4096, 2000, 0.1, 0.01, 42);
    let mask = create_detection_mask(&pixels, 4.0);

    b.bench(|| black_box(LabelMap::from_mask(black_box(&mask))));
}

#[quick_bench(warmup_iters = 1, iters = 10)]
fn bench_label_map_from_mask_6k_globular(b: ::bench::Bencher) {
    let pixels = benchmark_star_field(4096, 4096, 50000, 0.1, 0.01, 42);
    let mask = create_detection_mask(&pixels, 4.0);

    b.bench(|| black_box(LabelMap::from_mask(black_box(&mask))));
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

/// Benchmark run extraction - compares CTZ-based vs bit-by-bit scanning.
#[test]
#[ignore]
fn bench_run_extraction() {
    use super::{Run, extract_runs_from_row};
    use std::time::Instant;

    // Original bit-by-bit implementation for comparison
    #[inline]
    fn extract_runs_bitbybit(
        mask_words: &[u64],
        word_row_start: usize,
        words_per_row: usize,
        width: usize,
        runs: &mut Vec<Run>,
    ) {
        let mut in_run = false;
        let mut run_start = 0u32;

        for word_idx in 0..words_per_row {
            let word = mask_words[word_row_start + word_idx];
            let base_x = (word_idx * 64) as u32;

            if word == 0 {
                if in_run {
                    let end = base_x.min(width as u32);
                    runs.push(Run {
                        start: run_start,
                        end,
                        label: 0,
                    });
                    in_run = false;
                }
                continue;
            }

            if word == !0u64 {
                if !in_run {
                    run_start = base_x;
                    in_run = true;
                }
                continue;
            }

            // Mixed word - process bit by bit
            for bit in 0..64u32 {
                let x = base_x + bit;
                if x >= width as u32 {
                    break;
                }

                let is_set = (word >> bit) & 1 != 0;
                if is_set && !in_run {
                    run_start = x;
                    in_run = true;
                } else if !is_set && in_run {
                    runs.push(Run {
                        start: run_start,
                        end: x,
                        label: 0,
                    });
                    in_run = false;
                }
            }
        }

        if in_run {
            runs.push(Run {
                start: run_start,
                end: width as u32,
                label: 0,
            });
        }
    }

    println!("\n=== Run Extraction Benchmark: CTZ vs Bit-by-bit ===\n");
    println!(
        "{:>12} {:>8} {:>10} {:>12} {:>12} {:>8}",
        "Scenario", "Density", "Runs/row", "Bit-by-bit", "CTZ", "Speedup"
    );
    println!("{}", "-".repeat(72));

    let test_cases = [
        ("Sparse", 0.02f32),      // 2% density - typical star detection
        ("Medium", 0.10f32),      // 10% density
        ("Dense", 0.30f32),       // 30% density
        ("Very dense", 0.50f32),  // 50% density
        ("Alternating", -1.0f32), // Special: alternating bits (worst case)
    ];

    let width: usize = 4096;
    let words_per_row = width.div_ceil(64);

    for (name, density) in test_cases {
        // Create test data
        let mask_words: Vec<u64> = if density < 0.0 {
            // Alternating pattern: 0xAAAAAAAAAAAAAAAA
            vec![0xAAAAAAAAAAAAAAAAu64; words_per_row]
        } else {
            (0..words_per_row)
                .map(|i| {
                    let mut word = 0u64;
                    for bit in 0..64 {
                        let hash = ((i * 64 + bit) as u64)
                            .wrapping_mul(0x9E3779B97F4A7C15)
                            .wrapping_add(42);
                        if (hash as f32 / u64::MAX as f32) < density {
                            word |= 1u64 << bit;
                        }
                    }
                    word
                })
                .collect()
        };

        // Warmup both implementations
        let mut runs = Vec::with_capacity(width);
        for _ in 0..10 {
            runs.clear();
            extract_runs_bitbybit(&mask_words, 0, words_per_row, width, &mut runs);
            runs.clear();
            extract_runs_from_row(&mask_words, 0, words_per_row, width, &mut runs);
        }

        let iterations = 1000;

        // Benchmark bit-by-bit
        let start = Instant::now();
        for _ in 0..iterations {
            runs.clear();
            extract_runs_bitbybit(black_box(&mask_words), 0, words_per_row, width, &mut runs);
            black_box(&runs);
        }
        let bitbybit_time = start.elapsed() / iterations;

        // Benchmark CTZ
        let start = Instant::now();
        for _ in 0..iterations {
            runs.clear();
            extract_runs_from_row(black_box(&mask_words), 0, words_per_row, width, &mut runs);
            black_box(&runs);
        }
        let ctz_time = start.elapsed() / iterations;

        // Count runs
        runs.clear();
        extract_runs_from_row(&mask_words, 0, words_per_row, width, &mut runs);
        let run_count = runs.len();

        let speedup = bitbybit_time.as_nanos() as f64 / ctz_time.as_nanos() as f64;

        println!(
            "{:>12} {:>7.0}% {:>10} {:>10.2}µs {:>10.2}µs {:>7.2}x",
            name,
            if density < 0.0 { 50.0 } else { density * 100.0 },
            run_count,
            bitbybit_time.as_nanos() as f64 / 1000.0,
            ctz_time.as_nanos() as f64 / 1000.0,
            speedup
        );
    }
}
