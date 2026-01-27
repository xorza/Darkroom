//! Benchmark module for GPU sigma clipping and batch pipeline.
//! Run with: cargo bench -p lumos --features bench --bench stack_gpu_sigma_clip

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, Throughput};

use super::batch_pipeline::{BatchPipeline, BatchPipelineConfig};
use super::sigma_clip::{GpuSigmaClipConfig, GpuSigmaClipper};
use crate::stacking::SigmaClipConfig;

/// Register GPU stacking benchmarks with Criterion.
pub fn benchmarks(c: &mut Criterion) {
    benchmark_gpu_vs_cpu_sigma_clip(c);
    benchmark_batch_pipeline_throughput(c);
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

/// Benchmark batch pipeline end-to-end throughput.
///
/// This measures:
/// - Single batch processing (≤128 frames)
/// - Multi-batch processing with batch combination
/// - Sync vs async multi-batch performance
fn benchmark_batch_pipeline_throughput(c: &mut Criterion) {
    let pipeline = BatchPipeline::default();

    // Skip benchmark if no GPU available
    if !pipeline.gpu_available() {
        eprintln!("Skipping batch pipeline benchmark: no GPU available");
        return;
    }

    let mut group = c.benchmark_group("batch_pipeline");

    // Test various configurations
    // Image sizes represent typical astrophotography resolutions
    let sizes = [(1024, 1024), (2048, 2048), (4096, 4096)];

    // Frame counts: single batch, multi-batch (>128)
    let frame_counts = [32, 64, 128, 256];

    for (width, height) in sizes {
        for frame_count in frame_counts {
            let pixels_per_frame = width * height;
            let total_pixels = pixels_per_frame * frame_count;

            // Create test frames with realistic variation
            let frames: Vec<Vec<f32>> = (0..frame_count)
                .map(|i| {
                    // Simulate sky background with slight variation between frames
                    let base_value = 100.0 + (i as f32 * 0.5);
                    vec![base_value; pixels_per_frame]
                })
                .collect();
            let frame_refs: Vec<&[f32]> = frames.iter().map(|f| f.as_slice()).collect();

            let label = format!("{}x{}x{}", width, height, frame_count);

            // Set throughput as total pixels processed
            group.throughput(Throughput::Elements(total_pixels as u64));

            // Sync batch pipeline benchmark
            let config = BatchPipelineConfig::default();
            let mut pipeline_sync = BatchPipeline::new(config.clone());
            group.bench_function(BenchmarkId::new("sync", &label), |b| {
                b.iter(|| {
                    let result = pipeline_sync.stack(
                        black_box(&frame_refs),
                        black_box(width),
                        black_box(height),
                    );
                    black_box(result)
                })
            });

            // Async batch pipeline benchmark (only meaningful for multi-batch)
            if frame_count > 128 {
                let mut pipeline_async = BatchPipeline::new(config);
                group.bench_function(BenchmarkId::new("async", &label), |b| {
                    b.iter(|| {
                        let result = pipeline_async.stack_async(
                            black_box(&frame_refs),
                            black_box(width),
                            black_box(height),
                        );
                        black_box(result)
                    })
                });
            }
        }
    }

    group.finish();

    // Additional benchmark: throughput per frame for large stacks
    benchmark_large_stack_throughput(c);
}

/// Benchmark throughput for large frame counts (>128).
///
/// Measures frames per second throughput for different batch sizes.
fn benchmark_large_stack_throughput(c: &mut Criterion) {
    let pipeline = BatchPipeline::default();

    if !pipeline.gpu_available() {
        return;
    }

    let mut group = c.benchmark_group("large_stack_throughput");

    // Fixed image size (typical 4K astrophotography)
    let width = 2048;
    let height = 2048;
    let pixels_per_frame = width * height;

    // Large frame counts
    let frame_counts = [256, 384];

    // Different batch sizes to compare
    let batch_sizes = [32, 64, 128];

    for frame_count in frame_counts {
        let total_pixels = pixels_per_frame * frame_count;

        // Create test frames
        let frames: Vec<Vec<f32>> = (0..frame_count)
            .map(|i| vec![50.0 + (i as f32 * 0.2); pixels_per_frame])
            .collect();
        let frame_refs: Vec<&[f32]> = frames.iter().map(|f| f.as_slice()).collect();

        for batch_size in batch_sizes {
            let label = format!("{}frames_batch{}", frame_count, batch_size);

            group.throughput(Throughput::Elements(total_pixels as u64));

            let config = BatchPipelineConfig::default().batch_size(batch_size);
            let mut pipeline = BatchPipeline::new(config);

            group.bench_function(BenchmarkId::new("sync", &label), |b| {
                b.iter(|| {
                    let result =
                        pipeline.stack(black_box(&frame_refs), black_box(width), black_box(height));
                    black_box(result)
                })
            });
        }
    }

    group.finish();
}
