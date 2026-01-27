//! Benchmark module for GPU sigma clipping.
//! Run with: cargo bench -p lumos --features bench --bench stack_gpu_sigma_clip

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, Throughput};

use super::sigma_clip::{GpuSigmaClipConfig, GpuSigmaClipper};
use crate::stacking::SigmaClipConfig;

/// Register GPU stacking benchmarks with Criterion.
pub fn benchmarks(c: &mut Criterion) {
    benchmark_gpu_vs_cpu_sigma_clip(c);
}

/// Benchmark GPU vs CPU sigma clipping.
fn benchmark_gpu_vs_cpu_sigma_clip(c: &mut Criterion) {
    let mut clipper = GpuSigmaClipper::new();

    // Skip benchmark if no GPU available
    if !clipper.gpu_available() {
        eprintln!("Skipping GPU benchmark: no GPU available");
        return;
    }

    let mut group = c.benchmark_group("gpu_sigma_clip");

    // Test various image sizes and frame counts
    let sizes = [(256, 256), (512, 512), (1024, 1024), (2048, 2048)];
    let frame_counts = [10, 30, 50];

    for (width, height) in sizes {
        for frame_count in frame_counts {
            let pixels_per_frame = width * height;
            let total_pixels = pixels_per_frame * frame_count;

            // Create test frames with some variation and one outlier frame
            let frames: Vec<Vec<f32>> = (0..frame_count)
                .map(|i| {
                    if i == frame_count - 1 {
                        // Last frame is outlier
                        vec![1000.0f32; pixels_per_frame]
                    } else {
                        vec![10.0 + (i as f32 * 0.1); pixels_per_frame]
                    }
                })
                .collect();
            let frame_refs: Vec<&[f32]> = frames.iter().map(|f| f.as_slice()).collect();

            let label = format!("{}x{}x{}", width, height, frame_count);

            group.throughput(Throughput::Elements(total_pixels as u64));

            // GPU benchmark
            let gpu_config = GpuSigmaClipConfig::new(2.5, 3);
            group.bench_function(BenchmarkId::new("gpu", &label), |b| {
                b.iter(|| {
                    let result = clipper.stack(
                        black_box(&frame_refs),
                        black_box(width),
                        black_box(height),
                        black_box(&gpu_config),
                    );
                    black_box(result)
                })
            });

            // CPU benchmark (per-pixel sigma clipping)
            let cpu_config = SigmaClipConfig::new(2.5, 3);
            group.bench_function(BenchmarkId::new("cpu_perpixel", &label), |b| {
                b.iter(|| {
                    // Simulate CPU sigma clipping per pixel
                    let mut result = vec![0.0f32; pixels_per_frame];
                    for pixel_idx in 0..pixels_per_frame {
                        let mut values: Vec<f32> = frames.iter().map(|f| f[pixel_idx]).collect();
                        result[pixel_idx] = cpu_sigma_clipped_mean(&mut values, &cpu_config);
                    }
                    black_box(result)
                })
            });
        }
    }

    group.finish();
}

/// Simple CPU sigma-clipped mean for benchmarking (replicates the algorithm).
fn cpu_sigma_clipped_mean(values: &mut [f32], config: &SigmaClipConfig) -> f32 {
    if values.len() <= 2 {
        return values.iter().sum::<f32>() / values.len() as f32;
    }

    let mut len = values.len();

    for _ in 0..config.max_iterations {
        if len <= 2 {
            break;
        }

        let active = &mut values[..len];

        // Compute mean
        let mean = active.iter().sum::<f32>() / len as f32;
        let variance = active.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / len as f32;
        let std_dev = variance.sqrt();

        if std_dev < f32::EPSILON {
            break;
        }

        let threshold = config.sigma * std_dev;

        // Partition: move kept values to front
        let mut write_idx = 0;
        for read_idx in 0..len {
            if (values[read_idx] - mean).abs() <= threshold {
                values[write_idx] = values[read_idx];
                write_idx += 1;
            }
        }

        if write_idx == len {
            break;
        }
        len = write_idx;
    }

    values[..len].iter().sum::<f32>() / len as f32
}
