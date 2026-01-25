//! Benchmark module for Gaussian convolution and matched filtering.
//! Run with: cargo bench --package lumos --features bench convolution

use super::simd::convolve_row_simd;
use super::{fwhm_to_sigma, gaussian_convolve, gaussian_kernel_1d, matched_filter};
use criterion::{BenchmarkId, Criterion, Throughput};
use std::hint::black_box;

/// Generate a synthetic star field image for benchmarking.
fn generate_test_image(width: usize, height: usize) -> Vec<f32> {
    let mut pixels = vec![0.1f32; width * height];

    // Add deterministic "noise" pattern using simple hash
    for (i, p) in pixels.iter_mut().enumerate() {
        let hash = ((i as u32).wrapping_mul(2654435761)) as f32 / u32::MAX as f32;
        *p += (hash - 0.5) * 0.02;
    }

    // Add synthetic stars at deterministic positions
    let num_stars = (width * height) / 1000;
    for star_idx in 0..num_stars {
        let hash1 = ((star_idx as u32).wrapping_mul(2654435761)) as usize;
        let hash2 = ((star_idx as u32).wrapping_mul(1597334677)) as usize;
        let hash3 = ((star_idx as u32).wrapping_mul(805306457)) as usize;
        let hash4 = ((star_idx as u32).wrapping_mul(402653189)) as usize;

        let cx = 10 + (hash1 % (width - 20));
        let cy = 10 + (hash2 % (height - 20));
        let brightness = 0.3 + (hash3 % 600) as f32 / 1000.0;
        let sigma = 1.5 + (hash4 % 150) as f32 / 100.0;

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

/// Generate a flat background map for benchmarking.
fn generate_background(width: usize, height: usize) -> Vec<f32> {
    vec![0.1f32; width * height]
}

/// Register convolution benchmarks with Criterion.
pub fn benchmarks(c: &mut Criterion) {
    // Kernel generation benchmarks
    let mut kernel_group = c.benchmark_group("gaussian_kernel");
    kernel_group.sample_size(100);

    for sigma in [1.0, 2.0, 3.0, 4.0, 5.0] {
        kernel_group.bench_function(BenchmarkId::new("gaussian_kernel_1d", sigma), |b| {
            b.iter(|| black_box(gaussian_kernel_1d(black_box(sigma))))
        });
    }

    kernel_group.finish();

    // Gaussian convolution benchmarks
    let mut conv_group = c.benchmark_group("gaussian_convolve");
    conv_group.sample_size(20);

    // Test different image sizes
    for &(width, height) in &[(256, 256), (512, 512), (1024, 1024), (2048, 2048)] {
        let pixels = generate_test_image(width, height);
        let size_name = format!("{}x{}", width, height);

        conv_group.throughput(Throughput::Elements((width * height) as u64));

        // Test different sigma values (affects kernel size)
        for sigma in [1.0, 2.0, 3.0] {
            conv_group.bench_function(
                BenchmarkId::new(&size_name, format!("sigma_{}", sigma)),
                |b| {
                    b.iter(|| {
                        black_box(gaussian_convolve(
                            black_box(&pixels),
                            black_box(width),
                            black_box(height),
                            black_box(sigma),
                        ))
                    })
                },
            );
        }
    }

    conv_group.finish();

    // Matched filter benchmarks
    let mut filter_group = c.benchmark_group("matched_filter");
    filter_group.sample_size(20);

    for &(width, height) in &[(512, 512), (1024, 1024), (2048, 2048)] {
        let pixels = generate_test_image(width, height);
        let background = generate_background(width, height);
        let size_name = format!("{}x{}", width, height);

        filter_group.throughput(Throughput::Elements((width * height) as u64));

        // Test different FWHM values
        for fwhm in [3.0, 4.0, 5.0] {
            filter_group.bench_function(
                BenchmarkId::new(&size_name, format!("fwhm_{}", fwhm)),
                |b| {
                    b.iter(|| {
                        black_box(matched_filter(
                            black_box(&pixels),
                            black_box(width),
                            black_box(height),
                            black_box(&background),
                            black_box(fwhm),
                        ))
                    })
                },
            );
        }
    }

    filter_group.finish();

    // FWHM to sigma conversion benchmark
    let mut util_group = c.benchmark_group("convolution_utils");
    util_group.bench_function("fwhm_to_sigma", |b| {
        b.iter(|| {
            for fwhm in [2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0] {
                black_box(fwhm_to_sigma(black_box(fwhm)));
            }
        })
    });
    util_group.finish();

    // SIMD vs Scalar comparison for row convolution
    let mut simd_group = c.benchmark_group("convolution_simd_vs_scalar");
    simd_group.sample_size(50);

    for width in [256, 512, 1024, 2048] {
        let input: Vec<f32> = (0..width).map(|i| (i % 256) as f32 / 255.0).collect();
        let kernel = gaussian_kernel_1d(2.0); // typical sigma
        let radius = kernel.len() / 2;

        simd_group.throughput(Throughput::Elements(width as u64));

        // Scalar row convolution
        simd_group.bench_function(BenchmarkId::new("convolve_row_scalar", width), |b| {
            b.iter(|| {
                let mut output = vec![0.0f32; width];
                for (x, out) in output.iter_mut().enumerate() {
                    let mut sum = 0.0f32;
                    for (k, &kval) in kernel.iter().enumerate() {
                        let sx = x as isize + k as isize - radius as isize;
                        let sx = if sx < 0 {
                            (-sx) as usize
                        } else if sx >= width as isize {
                            2 * width - 2 - sx as usize
                        } else {
                            sx as usize
                        };
                        sum += black_box(&input)[sx] * kval;
                    }
                    *out = sum;
                }
                black_box(output)
            })
        });

        // SIMD row convolution
        simd_group.bench_function(BenchmarkId::new("convolve_row_simd", width), |b| {
            b.iter(|| {
                let mut output = vec![0.0f32; width];
                convolve_row_simd(
                    black_box(&input),
                    black_box(&mut output),
                    black_box(&kernel),
                    radius,
                );
                black_box(output)
            })
        });
    }

    simd_group.finish();
}
