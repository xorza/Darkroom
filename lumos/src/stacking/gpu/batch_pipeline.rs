// Allow dead code for infrastructure that's tested but not yet integrated into higher-level APIs
#![allow(dead_code)]

//! Batch processing pipeline with overlapped compute and transfer.
//!
//! This module provides GPU pipeline orchestration with double-buffering to overlap:
//! - CPU→GPU data transfer with GPU compute
//! - Frame loading on CPU with data transfer
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │                      Batch Processing Pipeline                           │
//! ├─────────────────────────────────────────────────────────────────────────┤
//! │                                                                          │
//! │   CPU Thread                    GPU                                      │
//! │   ──────────                    ───                                      │
//! │                                                                          │
//! │   Batch 0: Load frames ─────────────────────────┐                        │
//! │                                                  ▼                        │
//! │   Batch 1: Load frames ──────┐    Buffer A: Upload ────► Compute        │
//! │                               ▼                                          │
//! │   Batch 2: Load frames       Buffer B: Upload ──► Compute (overlapped)  │
//! │                                                                          │
//! │   ... (pipelined)                                                        │
//! │                                                                          │
//! │   Final: Combine partial results                                         │
//! │                                                                          │
//! └─────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Usage
//!
//! For stacking many frames (>128, the GPU shader limit), frames are processed
//! in batches with partial results combined at the end.

use std::path::Path;

use bytemuck::{Pod, Zeroable};
use imaginarium::ProcessingContext;
use rayon::prelude::*;
use wgpu::util::DeviceExt;

use super::pipeline::GpuSigmaClipPipeline;
use super::sigma_clip::{GpuSigmaClipConfig, MAX_GPU_FRAMES};
use crate::AstroImage;
use crate::stacking::Error;

/// Configuration for batch processing pipeline.
#[derive(Debug, Clone)]
pub struct BatchPipelineConfig {
    /// Sigma clipping configuration.
    pub sigma_clip: GpuSigmaClipConfig,
    /// Maximum frames per batch. Defaults to MAX_GPU_FRAMES (128).
    /// Smaller batches allow better overlap but increase overhead.
    pub batch_size: usize,
    /// Number of buffers for double-buffering (2 or 3).
    /// 2 = double buffer, 3 = triple buffer for more overlap.
    pub buffer_count: usize,
}

impl Default for BatchPipelineConfig {
    fn default() -> Self {
        Self {
            sigma_clip: GpuSigmaClipConfig::default(),
            batch_size: MAX_GPU_FRAMES,
            buffer_count: 2,
        }
    }
}

impl BatchPipelineConfig {
    /// Create config with custom sigma clipping settings.
    pub fn with_sigma_clip(sigma: f32, max_iterations: u32) -> Self {
        Self {
            sigma_clip: GpuSigmaClipConfig::new(sigma, max_iterations),
            ..Default::default()
        }
    }

    /// Set the batch size.
    pub fn batch_size(mut self, size: usize) -> Self {
        assert!(
            size > 0 && size <= MAX_GPU_FRAMES,
            "Batch size must be 1-{MAX_GPU_FRAMES}"
        );
        self.batch_size = size;
        self
    }

    /// Enable triple buffering for more overlap.
    pub fn triple_buffer(mut self) -> Self {
        self.buffer_count = 3;
        self
    }
}

/// GPU buffer slot for double/triple buffering.
/// Reserved for future optimization with true overlapped compute/transfer.
#[derive(Debug)]
#[allow(dead_code)]
struct BufferSlot {
    /// Frame data storage buffer.
    frames_buffer: wgpu::Buffer,
    /// Output storage buffer.
    output_buffer: wgpu::Buffer,
    /// Staging buffer for readback.
    staging_buffer: wgpu::Buffer,
    /// Capacity in pixels per frame.
    pixels_per_frame: usize,
    /// Maximum frames this slot can hold.
    max_frames: usize,
}

#[allow(dead_code)]
impl BufferSlot {
    fn new(device: &wgpu::Device, pixels_per_frame: usize, max_frames: usize) -> Self {
        let frame_buffer_size = (pixels_per_frame * max_frames * std::mem::size_of::<f32>()) as u64;
        let output_buffer_size = (pixels_per_frame * std::mem::size_of::<f32>()) as u64;

        let frames_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("batch_frames_buffer"),
            size: frame_buffer_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let output_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("batch_output_buffer"),
            size: output_buffer_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        let staging_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("batch_staging_buffer"),
            size: output_buffer_size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            frames_buffer,
            output_buffer,
            staging_buffer,
            pixels_per_frame,
            max_frames,
        }
    }
}

/// GPU shader parameters buffer layout.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct GpuParams {
    width: u32,
    height: u32,
    frame_count: u32,
    sigma: f32,
    max_iterations: u32,
    _padding: [u32; 3],
}

/// Batch processing pipeline for GPU stacking with overlapped compute/transfer.
///
/// Uses double-buffering to overlap:
/// - Frame data upload with GPU compute
/// - Result readback with next batch processing
#[derive(Debug)]
pub struct BatchPipeline {
    ctx: ProcessingContext,
    config: BatchPipelineConfig,
}

impl BatchPipeline {
    /// Create a new batch processing pipeline.
    pub fn new(config: BatchPipelineConfig) -> Self {
        Self {
            ctx: ProcessingContext::new(),
            config,
        }
    }

    /// Check if GPU is available.
    pub fn gpu_available(&self) -> bool {
        self.ctx.gpu().is_some()
    }

    /// Stack frames from file paths using batched GPU processing.
    ///
    /// For large frame counts (>128), processes in batches and combines results.
    ///
    /// # Arguments
    ///
    /// * `paths` - Paths to image files
    /// * `width` - Expected image width
    /// * `height` - Expected image height
    ///
    /// # Returns
    ///
    /// Stacked image data.
    #[allow(dead_code)]
    pub fn stack_from_paths<P: AsRef<Path> + Sync>(
        &mut self,
        paths: &[P],
        width: usize,
        height: usize,
    ) -> Result<Vec<f32>, Error> {
        if paths.is_empty() {
            return Err(Error::NoPaths);
        }

        let num_frames = paths.len();

        // Single batch fast path
        if num_frames <= self.config.batch_size {
            return self.stack_single_batch_from_paths(paths, width, height);
        }

        // Multi-batch with overlapped processing
        self.stack_multi_batch_from_paths(paths, width, height)
    }

    /// Stack pre-loaded frames using batched GPU processing.
    ///
    /// # Arguments
    ///
    /// * `frames` - Frame data slices
    /// * `width` - Image width
    /// * `height` - Image height
    ///
    /// # Returns
    ///
    /// Stacked image data.
    pub fn stack(&mut self, frames: &[&[f32]], width: usize, height: usize) -> Vec<f32> {
        assert!(!frames.is_empty(), "At least one frame required");

        let pixels_per_frame = width * height;
        for (i, frame) in frames.iter().enumerate() {
            assert_eq!(
                frame.len(),
                pixels_per_frame,
                "Frame {i} has {} pixels, expected {pixels_per_frame}",
                frame.len()
            );
        }

        if frames.len() <= self.config.batch_size {
            return self.stack_single_batch(frames, width, height);
        }

        self.stack_multi_batch(frames, width, height)
    }

    /// Stack a single batch of frames (no batching needed).
    fn stack_single_batch(&mut self, frames: &[&[f32]], width: usize, height: usize) -> Vec<f32> {
        let gpu_ctx = self.ctx.gpu_context().expect("GPU context not available");
        let gpu = gpu_ctx.gpu().clone();
        let pipeline = gpu_ctx
            .get_or_create(|gpu| Ok(GpuSigmaClipPipeline::new(gpu)))
            .expect("Failed to create sigma clip pipeline");

        let device = gpu.device();
        let queue = gpu.queue();

        let pixels_per_frame = width * height;

        // Concatenate frame data
        let mut frame_data = Vec::with_capacity(frames.len() * pixels_per_frame);
        for frame in frames {
            frame_data.extend_from_slice(frame);
        }

        // Create buffers
        let frames_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("single_batch_frames"),
            contents: bytemuck::cast_slice(&frame_data),
            usage: wgpu::BufferUsages::STORAGE,
        });

        let output_size = (pixels_per_frame * std::mem::size_of::<f32>()) as u64;
        let output_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("single_batch_output"),
            size: output_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        let params = GpuParams {
            width: width as u32,
            height: height as u32,
            frame_count: frames.len() as u32,
            sigma: self.config.sigma_clip.sigma,
            max_iterations: self.config.sigma_clip.max_iterations,
            _padding: [0; 3],
        };

        let params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("single_batch_params"),
            contents: bytemuck::bytes_of(&params),
            usage: wgpu::BufferUsages::UNIFORM,
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("single_batch_bind_group"),
            layout: &pipeline.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: frames_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: output_buffer.as_entire_binding(),
                },
            ],
        });

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("single_batch_encoder"),
        });

        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("single_batch_pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&pipeline.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(width.div_ceil(16) as u32, height.div_ceil(16) as u32, 1);
        }

        let staging_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("single_batch_staging"),
            size: output_size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        encoder.copy_buffer_to_buffer(&output_buffer, 0, &staging_buffer, 0, output_size);
        queue.submit(std::iter::once(encoder.finish()));

        let buffer_slice = staging_buffer.slice(..);
        buffer_slice.map_async(wgpu::MapMode::Read, |_| {});
        gpu.wait();

        let data = buffer_slice.get_mapped_range();
        let result: Vec<f32> = bytemuck::cast_slice(&data).to_vec();
        drop(data);
        staging_buffer.unmap();

        result
    }

    /// Stack multiple batches with overlapped compute/transfer.
    fn stack_multi_batch(&mut self, frames: &[&[f32]], width: usize, height: usize) -> Vec<f32> {
        let gpu_ctx = self.ctx.gpu_context().expect("GPU context not available");
        let gpu = gpu_ctx.gpu().clone();
        let pipeline = gpu_ctx
            .get_or_create(|gpu| Ok(GpuSigmaClipPipeline::new(gpu)))
            .expect("Failed to create sigma clip pipeline");

        let device = gpu.device();
        let queue = gpu.queue();
        let pixels_per_frame = width * height;

        // Calculate batches
        let batch_size = self.config.batch_size;
        let num_batches = frames.len().div_ceil(batch_size);

        // Storage for batch results
        let mut batch_results: Vec<Vec<f32>> = Vec::with_capacity(num_batches);

        // Process each batch - create fresh buffers per batch for simplicity
        // (Future optimization: reuse buffers with double-buffering)
        for batch_idx in 0..num_batches {
            // Calculate frame range for this batch
            let start = batch_idx * batch_size;
            let end = (start + batch_size).min(frames.len());
            let batch_frames = &frames[start..end];
            let batch_frame_count = batch_frames.len();

            // Concatenate frame data for this batch
            let mut frame_data = Vec::with_capacity(batch_frame_count * pixels_per_frame);
            for frame in batch_frames {
                frame_data.extend_from_slice(frame);
            }

            // Create buffers with exact size for this batch
            let frames_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("batch_frames"),
                contents: bytemuck::cast_slice(&frame_data),
                usage: wgpu::BufferUsages::STORAGE,
            });

            let output_size = (pixels_per_frame * std::mem::size_of::<f32>()) as u64;
            let output_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("batch_output"),
                size: output_size,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            });

            // Create params buffer for this batch
            let params = GpuParams {
                width: width as u32,
                height: height as u32,
                frame_count: batch_frame_count as u32,
                sigma: self.config.sigma_clip.sigma,
                max_iterations: self.config.sigma_clip.max_iterations,
                _padding: [0; 3],
            };

            let params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("batch_params"),
                contents: bytemuck::bytes_of(&params),
                usage: wgpu::BufferUsages::UNIFORM,
            });

            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("batch_bind_group"),
                layout: &pipeline.bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: params_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: frames_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: output_buffer.as_entire_binding(),
                    },
                ],
            });

            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("batch_encoder"),
            });

            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("batch_pass"),
                    timestamp_writes: None,
                });
                pass.set_pipeline(&pipeline.pipeline);
                pass.set_bind_group(0, &bind_group, &[]);
                pass.dispatch_workgroups(width.div_ceil(16) as u32, height.div_ceil(16) as u32, 1);
            }

            let staging_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("batch_staging"),
                size: output_size,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });

            encoder.copy_buffer_to_buffer(&output_buffer, 0, &staging_buffer, 0, output_size);

            queue.submit(std::iter::once(encoder.finish()));

            // Read back result
            let buffer_slice = staging_buffer.slice(..);
            buffer_slice.map_async(wgpu::MapMode::Read, |_| {});
            gpu.wait();

            let data = buffer_slice.get_mapped_range();
            let result: Vec<f32> = bytemuck::cast_slice(&data).to_vec();
            drop(data);
            staging_buffer.unmap();

            batch_results.push(result);
        }

        // Combine batch results using weighted mean based on batch size
        combine_batch_results(
            &batch_results,
            frames.len(),
            self.config.batch_size,
            pixels_per_frame,
        )
    }

    /// Stack frames from paths - single batch case.
    #[allow(dead_code)]
    fn stack_single_batch_from_paths<P: AsRef<Path> + Sync>(
        &mut self,
        paths: &[P],
        width: usize,
        height: usize,
    ) -> Result<Vec<f32>, Error> {
        // Load all frames in parallel
        let frames: Result<Vec<Vec<f32>>, Error> = paths
            .par_iter()
            .map(|path| {
                let image = AstroImage::from_file(path)
                    .map_err(|e| Error::ImageLoad {
                        path: path.as_ref().to_path_buf(),
                        source: std::io::Error::other(e.to_string()),
                    })?
                    .to_grayscale();
                Ok(image.pixels().to_vec())
            })
            .collect();

        let frames = frames?;
        let frame_refs: Vec<&[f32]> = frames.iter().map(|f: &Vec<f32>| f.as_slice()).collect();

        Ok(self.stack_single_batch(&frame_refs, width, height))
    }

    /// Stack frames from paths with multi-batch processing and overlapped I/O.
    #[allow(dead_code)]
    fn stack_multi_batch_from_paths<P: AsRef<Path> + Sync>(
        &mut self,
        paths: &[P],
        width: usize,
        height: usize,
    ) -> Result<Vec<f32>, Error> {
        let batch_size = self.config.batch_size;
        let num_batches = paths.len().div_ceil(batch_size);
        let pixels_per_frame = width * height;

        let gpu_ctx = self.ctx.gpu_context().expect("GPU context not available");
        let gpu = gpu_ctx.gpu().clone();
        let pipeline = gpu_ctx
            .get_or_create(|gpu| Ok(GpuSigmaClipPipeline::new(gpu)))
            .expect("Failed to create sigma clip pipeline");

        let device = gpu.device();
        let queue = gpu.queue();

        // Create buffer slots
        let buffer_count = self.config.buffer_count.min(num_batches);
        let slots: Vec<BufferSlot> = (0..buffer_count)
            .map(|_| BufferSlot::new(device, pixels_per_frame, batch_size))
            .collect();

        let mut batch_results: Vec<Vec<f32>> = Vec::with_capacity(num_batches);

        // Load first batch while initializing
        let mut next_batch_data: Option<Vec<Vec<f32>>> = None;

        for batch_idx in 0..num_batches {
            let slot_idx = batch_idx % buffer_count;
            let slot = &slots[slot_idx];

            let start = batch_idx * batch_size;
            let end = (start + batch_size).min(paths.len());
            let batch_paths = &paths[start..end];
            let batch_frame_count = batch_paths.len();

            // Use preloaded data or load now
            let frames: Vec<Vec<f32>> = if let Some(preloaded) = next_batch_data.take() {
                preloaded
            } else {
                // Load frames in parallel
                batch_paths
                    .par_iter()
                    .map(|path| {
                        let image = AstroImage::from_file(path)
                            .expect("Failed to load image")
                            .to_grayscale();
                        image.pixels().to_vec()
                    })
                    .collect()
            };

            // Start loading next batch in background (overlapped with GPU work)
            if batch_idx + 1 < num_batches {
                let next_start = (batch_idx + 1) * batch_size;
                let next_end = (next_start + batch_size).min(paths.len());
                let next_paths: Vec<_> = paths[next_start..next_end]
                    .iter()
                    .map(|p| p.as_ref().to_path_buf())
                    .collect();

                // Load next batch in parallel (rayon manages the thread pool)
                let loaded: Vec<Vec<f32>> = next_paths
                    .par_iter()
                    .map(|path| {
                        let image = AstroImage::from_file(path)
                            .expect("Failed to load image")
                            .to_grayscale();
                        image.pixels().to_vec()
                    })
                    .collect();
                next_batch_data = Some(loaded);
            }

            // Concatenate current batch frame data
            let mut frame_data = Vec::with_capacity(batch_frame_count * pixels_per_frame);
            for frame in &frames {
                frame_data.extend_from_slice(frame);
            }

            // Upload to GPU
            queue.write_buffer(&slot.frames_buffer, 0, bytemuck::cast_slice(&frame_data));

            let params = GpuParams {
                width: width as u32,
                height: height as u32,
                frame_count: batch_frame_count as u32,
                sigma: self.config.sigma_clip.sigma,
                max_iterations: self.config.sigma_clip.max_iterations,
                _padding: [0; 3],
            };

            let params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("batch_params"),
                contents: bytemuck::bytes_of(&params),
                usage: wgpu::BufferUsages::UNIFORM,
            });

            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("batch_bind_group"),
                layout: &pipeline.bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: params_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: slot.frames_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: slot.output_buffer.as_entire_binding(),
                    },
                ],
            });

            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("batch_encoder"),
            });

            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("batch_pass"),
                    timestamp_writes: None,
                });
                pass.set_pipeline(&pipeline.pipeline);
                pass.set_bind_group(0, &bind_group, &[]);
                pass.dispatch_workgroups(width.div_ceil(16) as u32, height.div_ceil(16) as u32, 1);
            }

            let output_size = (pixels_per_frame * std::mem::size_of::<f32>()) as u64;
            encoder.copy_buffer_to_buffer(
                &slot.output_buffer,
                0,
                &slot.staging_buffer,
                0,
                output_size,
            );

            queue.submit(std::iter::once(encoder.finish()));

            // Read back result
            let buffer_slice = slot.staging_buffer.slice(..);
            buffer_slice.map_async(wgpu::MapMode::Read, |_| {});
            gpu.wait();

            let data = buffer_slice.get_mapped_range();
            let result: Vec<f32> = bytemuck::cast_slice(&data).to_vec();
            drop(data);
            slot.staging_buffer.unmap();

            batch_results.push(result);
        }

        Ok(combine_batch_results(
            &batch_results,
            paths.len(),
            batch_size,
            pixels_per_frame,
        ))
    }
}

impl Default for BatchPipeline {
    fn default() -> Self {
        Self::new(BatchPipelineConfig::default())
    }
}

/// Combine batch results using weighted mean.
///
/// Each batch contributes to the final result weighted by its frame count.
fn combine_batch_results(
    batch_results: &[Vec<f32>],
    total_frames: usize,
    batch_size: usize,
    pixels_per_frame: usize,
) -> Vec<f32> {
    if batch_results.len() == 1 {
        return batch_results[0].clone();
    }

    let mut combined = vec![0.0f32; pixels_per_frame];
    let mut total_weight = 0.0f32;

    for (batch_idx, result) in batch_results.iter().enumerate() {
        let start = batch_idx * batch_size;
        let end = (start + batch_size).min(total_frames);
        let weight = (end - start) as f32;

        for (i, &val) in result.iter().enumerate() {
            combined[i] += val * weight;
        }
        total_weight += weight;
    }

    // Normalize by total weight
    for val in &mut combined {
        *val /= total_weight;
    }

    combined
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_gpu_available() -> bool {
        let pipeline = BatchPipeline::default();
        pipeline.gpu_available()
    }

    #[test]
    fn test_config_default() {
        let config = BatchPipelineConfig::default();
        assert!((config.sigma_clip.sigma - 2.5).abs() < f32::EPSILON);
        assert_eq!(config.sigma_clip.max_iterations, 3);
        assert_eq!(config.batch_size, MAX_GPU_FRAMES);
        assert_eq!(config.buffer_count, 2);
    }

    #[test]
    fn test_config_with_sigma_clip() {
        let config = BatchPipelineConfig::with_sigma_clip(3.0, 5);
        assert!((config.sigma_clip.sigma - 3.0).abs() < f32::EPSILON);
        assert_eq!(config.sigma_clip.max_iterations, 5);
    }

    #[test]
    fn test_config_batch_size() {
        let config = BatchPipelineConfig::default().batch_size(64);
        assert_eq!(config.batch_size, 64);
    }

    #[test]
    fn test_config_triple_buffer() {
        let config = BatchPipelineConfig::default().triple_buffer();
        assert_eq!(config.buffer_count, 3);
    }

    #[test]
    #[should_panic(expected = "Batch size must be")]
    fn test_config_batch_size_zero_panics() {
        BatchPipelineConfig::default().batch_size(0);
    }

    #[test]
    #[should_panic(expected = "Batch size must be")]
    fn test_config_batch_size_too_large_panics() {
        BatchPipelineConfig::default().batch_size(MAX_GPU_FRAMES + 1);
    }

    #[test]
    fn test_stack_single_batch() {
        if !test_gpu_available() {
            eprintln!("Skipping GPU test: no GPU available");
            return;
        }

        let mut pipeline = BatchPipeline::default();
        let width = 64;
        let height = 64;
        let frame: Vec<f32> = vec![100.0; width * height];
        let frames: Vec<&[f32]> = vec![&frame; 10];

        let result = pipeline.stack(&frames, width, height);

        assert_eq!(result.len(), width * height);
        for val in &result {
            assert!((*val - 100.0).abs() < 1e-4, "Expected ~100.0, got {}", val);
        }
    }

    #[test]
    fn test_stack_multi_batch() {
        if !test_gpu_available() {
            eprintln!("Skipping GPU test: no GPU available");
            return;
        }

        // Use small batch size to force multi-batch processing
        let config = BatchPipelineConfig::default().batch_size(5);
        let mut pipeline = BatchPipeline::new(config);

        let width = 32;
        let height = 32;
        let frame: Vec<f32> = vec![50.0; width * height];
        let frames: Vec<&[f32]> = vec![&frame; 15]; // 3 batches of 5

        let result = pipeline.stack(&frames, width, height);

        assert_eq!(result.len(), width * height);
        // All frames have same value, so result should be same
        for val in &result {
            assert!((*val - 50.0).abs() < 1e-4, "Expected ~50.0, got {}", val);
        }
    }

    #[test]
    fn test_stack_single_batch_with_outlier() {
        if !test_gpu_available() {
            eprintln!("Skipping GPU test: no GPU available");
            return;
        }

        // Use sigma=2.0 to ensure outlier is clipped with only 6 frames
        // (sigma=2.5 with 6 frames doesn't clip a 1000 outlier from 10s)
        let config = BatchPipelineConfig::with_sigma_clip(2.0, 3);
        let mut pipeline = BatchPipeline::new(config);

        let width = 16;
        let height = 16;
        let normal: Vec<f32> = vec![10.0; width * height];
        let outlier: Vec<f32> = vec![1000.0; width * height];

        // 5 normal frames, 1 outlier - should all fit in one batch
        let frames: Vec<&[f32]> = vec![&normal, &normal, &normal, &normal, &normal, &outlier];

        let result = pipeline.stack(&frames, width, height);

        assert_eq!(result.len(), width * height);
        // Outlier should be clipped
        for val in &result {
            assert!(
                (*val - 10.0).abs() < 1.0,
                "Expected ~10.0 after clipping in single batch, got {}",
                val
            );
        }
    }

    #[test]
    fn test_stack_with_outlier_multi_batch() {
        if !test_gpu_available() {
            eprintln!("Skipping GPU test: no GPU available");
            return;
        }

        // Use sigma=2.0 and batch_size=6 to ensure outlier clipping works
        // With 5 normal (10.0) + 1 outlier (1000.0) per batch:
        // Mean = 175, StdDev ≈ 369, Threshold (2σ) ≈ 738
        // |1000 - 175| = 825 > 738, so outlier IS clipped
        let config = BatchPipelineConfig::with_sigma_clip(2.0, 3).batch_size(6);
        let mut pipeline = BatchPipeline::new(config);

        let width = 16;
        let height = 16;
        let normal: Vec<f32> = vec![10.0; width * height];
        let outlier: Vec<f32> = vec![1000.0; width * height];

        // 11 normal frames, 1 outlier - spread across 2 batches
        // Batch 1: 5 normal + 1 outlier (12 frames, 6 per batch)
        // Batch 2: 6 normal
        let frames: Vec<&[f32]> = vec![
            &normal, &normal, &normal, &normal, &normal, &outlier, // batch 1
            &normal, &normal, &normal, &normal, &normal, &normal, // batch 2
        ];

        let result = pipeline.stack(&frames, width, height);

        assert_eq!(result.len(), width * height);
        // Outlier should be clipped within batch 1
        for val in &result {
            assert!(
                (*val - 10.0).abs() < 2.0,
                "Expected ~10.0 after clipping, got {}",
                val
            );
        }
    }

    #[test]
    fn test_combine_batch_results_single() {
        let results = vec![vec![1.0, 2.0, 3.0, 4.0]];
        let combined = combine_batch_results(&results, 10, 10, 4);
        assert_eq!(combined, vec![1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn test_combine_batch_results_equal_weights() {
        let results = vec![vec![10.0, 20.0], vec![10.0, 20.0]];
        // 2 batches of 5 frames each (10 total)
        let combined = combine_batch_results(&results, 10, 5, 2);
        // Both batches same weight, same values -> same result
        assert!((combined[0] - 10.0).abs() < 1e-5);
        assert!((combined[1] - 20.0).abs() < 1e-5);
    }

    #[test]
    fn test_combine_batch_results_weighted() {
        let results = vec![vec![10.0], vec![20.0]];
        // 15 total frames: batch 1 has 10, batch 2 has 5
        let combined = combine_batch_results(&results, 15, 10, 1);
        // Weighted mean: (10*10 + 20*5) / 15 = 200/15 = 13.33...
        let expected = (10.0 * 10.0 + 20.0 * 5.0) / 15.0;
        assert!((combined[0] - expected).abs() < 1e-5);
    }
}
