//! Benchmark module for phase correlation.
//! Run with: cargo bench -p lumos --features bench --bench registration_phase_corr

use std::hint::black_box;
use std::path::Path;

use criterion::{BenchmarkId, Criterion, Throughput};

use super::{PhaseCorrelationConfig, PhaseCorrelator, SubpixelMethod};

/// Register phase correlation benchmarks with Criterion.
pub fn benchmarks(c: &mut Criterion, _calibration_dir: &Path) {
    benchmark_phase_correlation_sizes(c);
    benchmark_subpixel_methods(c);
    benchmark_windowing(c);
    benchmark_iterative_correlation(c);
}

/// Generate a test image with a Gaussian spot.
fn generate_test_image(width: usize, height: usize, spot_x: f32, spot_y: f32) -> Vec<f32> {
    let mut image = vec![0.0f32; width * height];
    let sigma = 10.0f32;

    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - spot_x;
            let dy = y as f32 - spot_y;
            let value = (-((dx * dx + dy * dy) / (2.0 * sigma * sigma))).exp();
            image[y * width + x] = value;
        }
    }

    image
}

/// Benchmark phase correlation with different image sizes.
fn benchmark_phase_correlation_sizes(c: &mut Criterion) {
    let mut group = c.benchmark_group("phase_correlation_sizes");

    let config = PhaseCorrelationConfig::default();

    for size in [128, 256, 512, 1024] {
        let ref_image = generate_test_image(size, size, size as f32 / 2.0, size as f32 / 2.0);
        let target_image =
            generate_test_image(size, size, size as f32 / 2.0 + 5.0, size as f32 / 2.0 + 3.0);

        let correlator = PhaseCorrelator::new(size, size, config.clone());

        group.throughput(Throughput::Elements((size * size) as u64));

        group.bench_function(
            BenchmarkId::new("correlate", format!("{}x{}", size, size)),
            |b| {
                b.iter(|| {
                    let result = correlator.correlate(
                        black_box(&ref_image),
                        black_box(&target_image),
                        size,
                        size,
                    );
                    black_box(result)
                })
            },
        );
    }

    group.finish();
}

/// Benchmark different subpixel refinement methods.
fn benchmark_subpixel_methods(c: &mut Criterion) {
    let mut group = c.benchmark_group("phase_correlation_subpixel");

    let size = 256;
    let ref_image = generate_test_image(size, size, size as f32 / 2.0, size as f32 / 2.0);
    let target_image =
        generate_test_image(size, size, size as f32 / 2.0 + 5.3, size as f32 / 2.0 + 2.7);

    let methods = [
        ("none", SubpixelMethod::None),
        ("parabolic", SubpixelMethod::Parabolic),
        ("gaussian", SubpixelMethod::Gaussian),
        ("centroid", SubpixelMethod::Centroid),
    ];

    for (name, method) in methods {
        let config = PhaseCorrelationConfig {
            subpixel_method: method,
            ..Default::default()
        };
        let correlator = PhaseCorrelator::new(size, size, config);

        group.bench_function(name, |b| {
            b.iter(|| {
                let result = correlator.correlate(
                    black_box(&ref_image),
                    black_box(&target_image),
                    size,
                    size,
                );
                black_box(result)
            })
        });
    }

    group.finish();
}

/// Benchmark iterative correlation for sub-pixel accuracy improvement.
fn benchmark_iterative_correlation(c: &mut Criterion) {
    let mut group = c.benchmark_group("phase_correlation_iterative");

    // Create smooth multi-feature test image
    let size = 256;
    let ref_image = generate_multi_feature_image(size, size);

    // Sub-pixel shift
    let shift_x = 5.37f32;
    let shift_y = 3.21f32;
    let target_image = generate_shifted_image(&ref_image, size, size, shift_x, shift_y);

    // Standard single-shot correlation
    let config_single = PhaseCorrelationConfig {
        subpixel_method: SubpixelMethod::Gaussian,
        max_iterations: 0, // Disabled
        ..Default::default()
    };
    let correlator_single = PhaseCorrelator::new(size, size, config_single);

    group.bench_function("single_shot", |b| {
        b.iter(|| {
            let result = correlator_single.correlate(
                black_box(&ref_image),
                black_box(&target_image),
                size,
                size,
            );
            black_box(result)
        })
    });

    // Iterative correlation (3 iterations)
    let config_iter3 = PhaseCorrelationConfig {
        subpixel_method: SubpixelMethod::Gaussian,
        max_iterations: 3,
        convergence_threshold: 0.01,
        ..Default::default()
    };
    let correlator_iter3 = PhaseCorrelator::new(size, size, config_iter3);

    group.bench_function("iterative_3", |b| {
        b.iter(|| {
            let result = correlator_iter3.correlate_iterative(
                black_box(&ref_image),
                black_box(&target_image),
                size,
                size,
            );
            black_box(result)
        })
    });

    // Iterative correlation (5 iterations)
    let config_iter5 = PhaseCorrelationConfig {
        subpixel_method: SubpixelMethod::Gaussian,
        max_iterations: 5,
        convergence_threshold: 0.01,
        ..Default::default()
    };
    let correlator_iter5 = PhaseCorrelator::new(size, size, config_iter5);

    group.bench_function("iterative_5", |b| {
        b.iter(|| {
            let result = correlator_iter5.correlate_iterative(
                black_box(&ref_image),
                black_box(&target_image),
                size,
                size,
            );
            black_box(result)
        })
    });

    group.finish();
}

/// Generate a multi-feature image with multiple Gaussian spots (for iterative correlation testing).
fn generate_multi_feature_image(width: usize, height: usize) -> Vec<f32> {
    let mut image = vec![0.0f32; width * height];
    let sigma = 15.0f32;

    // Add multiple Gaussian spots
    let spots = [
        (width as f32 * 0.25, height as f32 * 0.25),
        (width as f32 * 0.75, height as f32 * 0.25),
        (width as f32 * 0.5, height as f32 * 0.5),
        (width as f32 * 0.25, height as f32 * 0.75),
        (width as f32 * 0.75, height as f32 * 0.75),
    ];

    for y in 0..height {
        for x in 0..width {
            let mut value = 0.0f32;
            for &(cx, cy) in &spots {
                let dx = x as f32 - cx;
                let dy = y as f32 - cy;
                value += (-((dx * dx + dy * dy) / (2.0 * sigma * sigma))).exp();
            }
            image[y * width + x] = value;
        }
    }

    image
}

/// Generate a shifted version of an image using bilinear interpolation.
fn generate_shifted_image(
    input: &[f32],
    width: usize,
    height: usize,
    shift_x: f32,
    shift_y: f32,
) -> Vec<f32> {
    let mut output = vec![0.0f32; width * height];

    for y in 0..height {
        for x in 0..width {
            let src_x = x as f32 - shift_x;
            let src_y = y as f32 - shift_y;

            if src_x >= 0.0
                && src_x < (width - 1) as f32
                && src_y >= 0.0
                && src_y < (height - 1) as f32
            {
                let x0 = src_x.floor() as usize;
                let y0 = src_y.floor() as usize;
                let fx = src_x - x0 as f32;
                let fy = src_y - y0 as f32;

                let p00 = input[y0 * width + x0];
                let p10 = input[y0 * width + x0 + 1];
                let p01 = input[(y0 + 1) * width + x0];
                let p11 = input[(y0 + 1) * width + x0 + 1];

                let top = p00 + fx * (p10 - p00);
                let bottom = p01 + fx * (p11 - p01);
                output[y * width + x] = top + fy * (bottom - top);
            }
        }
    }

    output
}

/// Benchmark effect of windowing on performance.
fn benchmark_windowing(c: &mut Criterion) {
    let mut group = c.benchmark_group("phase_correlation_windowing");

    let size = 512;
    let ref_image = generate_test_image(size, size, size as f32 / 2.0, size as f32 / 2.0);
    let target_image = generate_test_image(
        size,
        size,
        size as f32 / 2.0 + 10.0,
        size as f32 / 2.0 - 5.0,
    );

    for use_windowing in [false, true] {
        let config = PhaseCorrelationConfig {
            use_windowing,
            ..Default::default()
        };
        let correlator = PhaseCorrelator::new(size, size, config);

        let name = if use_windowing {
            "with_window"
        } else {
            "no_window"
        };

        group.bench_function(name, |b| {
            b.iter(|| {
                let result = correlator.correlate(
                    black_box(&ref_image),
                    black_box(&target_image),
                    size,
                    size,
                );
                black_box(result)
            })
        });
    }

    group.finish();
}
