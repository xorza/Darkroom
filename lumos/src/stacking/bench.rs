//! Benchmarks for image stacking.
//!
//! Run with: `cargo test -p lumos --release bench_stack -- --ignored --nocapture`

use bench::quick_bench;
use std::hint::black_box;

use crate::astro_image::{AstroImage, ImageDimensions};
use crate::stacking::cache::tests::make_test_cache;
use crate::stacking::config::StackConfig;
use crate::stacking::stack::run_stacking;

/// Image dimensions for stacking benchmarks.
/// 1920×1280 is a realistic sub-frame size (close to a binned DSLR frame).
const BENCH_WIDTH: usize = 1920;
const BENCH_HEIGHT: usize = 1280;

/// Generate synthetic bias frames (grayscale).
///
/// Bias frames have a constant pedestal (e.g. 1000 ADU) plus readout noise.
/// Each frame gets a slightly different pedestal to simulate real variation.
fn make_bias_frames(count: usize) -> Vec<AstroImage> {
    let dims = ImageDimensions::new(BENCH_WIDTH, BENCH_HEIGHT, 1);
    let npix = BENCH_WIDTH * BENCH_HEIGHT;

    (0..count)
        .map(|i| {
            // Base pedestal varies slightly per frame (1000 ± 2)
            let pedestal = 1000.0 + (i % 5) as f32 * 0.8 - 1.6;
            let mut pixels = vec![pedestal; npix];
            // Add deterministic pseudo-noise pattern
            for (j, p) in pixels.iter_mut().enumerate() {
                // Simple hash-based noise: ±5 ADU readout noise
                let hash = ((j as u32).wrapping_mul(2654435761) ^ (i as u32 * 31)) as f32;
                *p += (hash / u32::MAX as f32 - 0.5) * 10.0;
            }
            AstroImage::from_pixels(dims, pixels)
        })
        .collect()
}

/// Generate synthetic dark frames (grayscale).
///
/// Dark frames have a pedestal + thermal noise that increases with position
/// (hot pixel gradient). Some frames have hot pixels (outliers).
fn make_dark_frames(count: usize) -> Vec<AstroImage> {
    let dims = ImageDimensions::new(BENCH_WIDTH, BENCH_HEIGHT, 1);
    let npix = BENCH_WIDTH * BENCH_HEIGHT;

    (0..count)
        .map(|i| {
            let pedestal = 1000.0 + (i % 7) as f32 * 0.5 - 1.5;
            let mut pixels = vec![0.0f32; npix];
            for y in 0..BENCH_HEIGHT {
                for x in 0..BENCH_WIDTH {
                    let idx = y * BENCH_WIDTH + x;
                    // Thermal gradient: increases toward bottom-right
                    let thermal = (y as f32 / BENCH_HEIGHT as f32) * 20.0;
                    // Pseudo-random noise
                    let hash = ((idx as u32).wrapping_mul(2654435761) ^ (i as u32 * 37)) as f32;
                    let noise = (hash / u32::MAX as f32 - 0.5) * 8.0;
                    pixels[idx] = pedestal + thermal + noise;
                }
            }
            // Add ~0.1% hot pixels
            let hot_count = npix / 1000;
            for h in 0..hot_count {
                let idx = ((h as u32).wrapping_mul(2654435761) ^ (i as u32 * 53)) as usize % npix;
                pixels[idx] = 10000.0 + (h as f32) * 100.0;
            }
            AstroImage::from_pixels(dims, pixels)
        })
        .collect()
}

/// Generate synthetic flat frames (RGB).
///
/// Flat frames have a vignetting pattern (bright center, dim corners)
/// with multiplicative variation between frames.
fn make_flat_frames(count: usize) -> Vec<AstroImage> {
    let dims = ImageDimensions::new(BENCH_WIDTH, BENCH_HEIGHT, 3);
    let npix = BENCH_WIDTH * BENCH_HEIGHT;
    let cx = BENCH_WIDTH as f32 / 2.0;
    let cy = BENCH_HEIGHT as f32 / 2.0;
    let max_r2 = cx * cx + cy * cy;

    (0..count)
        .map(|i| {
            // Flat level varies ±10% between frames (exposure variation)
            let level = 30000.0 * (1.0 + ((i % 5) as f32 - 2.0) * 0.04);
            let mut r_ch = vec![0.0f32; npix];
            let mut g_ch = vec![0.0f32; npix];
            let mut b_ch = vec![0.0f32; npix];

            for y in 0..BENCH_HEIGHT {
                for x in 0..BENCH_WIDTH {
                    let idx = y * BENCH_WIDTH + x;
                    let dx = x as f32 - cx;
                    let dy = y as f32 - cy;
                    let r2 = dx * dx + dy * dy;
                    // Vignetting: 100% center, 60% corners
                    let vignette = 1.0 - 0.4 * (r2 / max_r2);
                    // Small per-channel variation
                    let hash = ((idx as u32).wrapping_mul(2654435761) ^ (i as u32 * 41)) as f32;
                    let noise = (hash / u32::MAX as f32 - 0.5) * 50.0;
                    r_ch[idx] = level * vignette * 1.02 + noise;
                    g_ch[idx] = level * vignette * 1.00 + noise;
                    b_ch[idx] = level * vignette * 0.98 + noise;
                }
            }
            AstroImage::from_planar_channels(dims, vec![r_ch, g_ch, b_ch])
        })
        .collect()
}

/// Generate synthetic light frames (RGB).
///
/// Light frames have a sky background gradient + stars + noise.
/// Global normalization is needed because sky levels vary between frames.
fn make_light_frames(count: usize) -> Vec<AstroImage> {
    let dims = ImageDimensions::new(BENCH_WIDTH, BENCH_HEIGHT, 3);
    let npix = BENCH_WIDTH * BENCH_HEIGHT;

    (0..count)
        .map(|i| {
            // Sky level varies between frames (±15%)
            let sky = 500.0 * (1.0 + ((i % 7) as f32 - 3.0) * 0.05);
            let mut r_ch = vec![0.0f32; npix];
            let mut g_ch = vec![0.0f32; npix];
            let mut b_ch = vec![0.0f32; npix];

            for y in 0..BENCH_HEIGHT {
                for x in 0..BENCH_WIDTH {
                    let idx = y * BENCH_WIDTH + x;
                    // Sky gradient (brighter at bottom from light pollution)
                    let gradient = sky + (y as f32 / BENCH_HEIGHT as f32) * 100.0;
                    // Noise
                    let hash = ((idx as u32).wrapping_mul(2654435761) ^ (i as u32 * 43)) as f32;
                    let noise = (hash / u32::MAX as f32 - 0.5) * 30.0;
                    r_ch[idx] = gradient * 1.0 + noise;
                    g_ch[idx] = gradient * 0.9 + noise;
                    b_ch[idx] = gradient * 0.8 + noise;
                }
            }
            // Add ~200 fake stars as bright points
            for s in 0..200 {
                let sx =
                    ((s as u32).wrapping_mul(2654435761) ^ (i as u32 * 59)) as usize % BENCH_WIDTH;
                let sy =
                    ((s as u32).wrapping_mul(1597334677) ^ (i as u32 * 67)) as usize % BENCH_HEIGHT;
                let brightness = 5000.0 + (s as f32) * 200.0;
                // 3×3 star footprint
                for dy in 0..3_usize {
                    for dx in 0..3_usize {
                        let px = sx.wrapping_add(dx).wrapping_sub(1);
                        let py = sy.wrapping_add(dy).wrapping_sub(1);
                        if px < BENCH_WIDTH && py < BENCH_HEIGHT {
                            let idx = py * BENCH_WIDTH + px;
                            let w = if dx == 1 && dy == 1 { 1.0 } else { 0.3 };
                            r_ch[idx] += brightness * w;
                            g_ch[idx] += brightness * w;
                            b_ch[idx] += brightness * w;
                        }
                    }
                }
            }
            AstroImage::from_planar_channels(dims, vec![r_ch, g_ch, b_ch])
        })
        .collect()
}

// ========== Bias Stacking Benchmarks ==========

#[quick_bench(warmup_iters = 1, iters = 3)]
fn bench_stack_bias_10(b: ::bench::Bencher) {
    let frames = make_bias_frames(10);
    let cache = make_test_cache(frames);
    let config = StackConfig::bias();

    b.bench(|| black_box(run_stacking(&cache, &config)));
}

#[quick_bench(warmup_iters = 1, iters = 3)]
fn bench_stack_bias_30(b: ::bench::Bencher) {
    let frames = make_bias_frames(30);
    let cache = make_test_cache(frames);
    let config = StackConfig::bias();

    b.bench(|| black_box(run_stacking(&cache, &config)));
}

#[quick_bench(warmup_iters = 1, iters = 3)]
fn bench_stack_bias_100(b: ::bench::Bencher) {
    let frames = make_bias_frames(100);
    let cache = make_test_cache(frames);
    let config = StackConfig::bias();

    b.bench(|| black_box(run_stacking(&cache, &config)));
}

// ========== Dark Stacking Benchmarks ==========

#[quick_bench(warmup_iters = 1, iters = 3)]
fn bench_stack_dark_10(b: ::bench::Bencher) {
    let frames = make_dark_frames(10);
    let cache = make_test_cache(frames);
    let config = StackConfig::dark();

    b.bench(|| black_box(run_stacking(&cache, &config)));
}

#[quick_bench(warmup_iters = 1, iters = 3)]
fn bench_stack_dark_30(b: ::bench::Bencher) {
    let frames = make_dark_frames(30);
    let cache = make_test_cache(frames);
    let config = StackConfig::dark();

    b.bench(|| black_box(run_stacking(&cache, &config)));
}

#[quick_bench(warmup_iters = 1, iters = 3)]
fn bench_stack_dark_100(b: ::bench::Bencher) {
    let frames = make_dark_frames(100);
    let cache = make_test_cache(frames);
    let config = StackConfig::dark();

    b.bench(|| black_box(run_stacking(&cache, &config)));
}

// ========== Flat Stacking Benchmarks ==========

#[quick_bench(warmup_iters = 1, iters = 3)]
fn bench_stack_flat_10(b: ::bench::Bencher) {
    let frames = make_flat_frames(10);
    let cache = make_test_cache(frames);
    let config = StackConfig::flat();

    b.bench(|| black_box(run_stacking(&cache, &config)));
}

#[quick_bench(warmup_iters = 1, iters = 3)]
fn bench_stack_flat_30(b: ::bench::Bencher) {
    let frames = make_flat_frames(30);
    let cache = make_test_cache(frames);
    let config = StackConfig::flat();

    b.bench(|| black_box(run_stacking(&cache, &config)));
}

#[quick_bench(warmup_iters = 1, iters = 3)]
fn bench_stack_flat_100(b: ::bench::Bencher) {
    let frames = make_flat_frames(100);
    let cache = make_test_cache(frames);
    let config = StackConfig::flat();

    b.bench(|| black_box(run_stacking(&cache, &config)));
}

// ========== Light Stacking Benchmarks ==========

#[quick_bench(warmup_iters = 1, iters = 3)]
fn bench_stack_light_10(b: ::bench::Bencher) {
    let frames = make_light_frames(10);
    let cache = make_test_cache(frames);
    let config = StackConfig::light();

    b.bench(|| black_box(run_stacking(&cache, &config)));
}

#[quick_bench(warmup_iters = 1, iters = 3)]
fn bench_stack_light_30(b: ::bench::Bencher) {
    let frames = make_light_frames(30);
    let cache = make_test_cache(frames);
    let config = StackConfig::light();

    b.bench(|| black_box(run_stacking(&cache, &config)));
}

#[quick_bench(warmup_iters = 1, iters = 3)]
fn bench_stack_light_100(b: ::bench::Bencher) {
    let frames = make_light_frames(100);
    let cache = make_test_cache(frames);
    let config = StackConfig::light();

    b.bench(|| black_box(run_stacking(&cache, &config)));
}
