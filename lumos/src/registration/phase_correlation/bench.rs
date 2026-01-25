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
