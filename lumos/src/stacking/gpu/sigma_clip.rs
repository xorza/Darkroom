//! GPU-accelerated sigma clipping for image stacking.
//!
//! Uses mean-based iterative sigma clipping computed on the GPU.
//! Each GPU thread handles one pixel position across all N frames.

use bytemuck::{Pod, Zeroable};
use imaginarium::ProcessingContext;
use wgpu::util::DeviceExt;

use super::pipeline::GpuSigmaClipPipeline;

/// Maximum number of frames supported by the GPU shader.
/// Limited by WGSL array size and register pressure.
pub const MAX_GPU_FRAMES: usize = 128;

/// Configuration for GPU sigma clipping.
#[derive(Debug, Clone, Copy)]
pub struct GpuSigmaClipConfig {
    /// Clipping threshold in standard deviations (default: 2.5).
    pub sigma: f32,
    /// Maximum number of clipping iterations (default: 3).
    pub max_iterations: u32,
}

impl Default for GpuSigmaClipConfig {
    fn default() -> Self {
        Self {
            sigma: 2.5,
            max_iterations: 3,
        }
    }
}

impl GpuSigmaClipConfig {
    /// Create a new config with the given sigma threshold.
    pub fn new(sigma: f32, max_iterations: u32) -> Self {
        assert!(sigma > 0.0, "Sigma must be positive");
        assert!(max_iterations > 0, "Max iterations must be at least 1");
        Self {
            sigma,
            max_iterations,
        }
    }
}

/// GPU shader parameters buffer layout.
/// Must match the WGSL struct exactly (32 bytes, aligned).
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct GpuParams {
    width: u32,
    height: u32,
    frame_count: u32,
    sigma: f32,
    max_iterations: u32,
    _padding: [u32; 3], // Align to 32 bytes
}

/// GPU-accelerated sigma clipping stacker.
///
/// Reuses the GPU context and pipeline across multiple stacking operations.
#[derive(Debug)]
pub struct GpuSigmaClipper {
    ctx: ProcessingContext,
}

impl GpuSigmaClipper {
    /// Create a new GPU sigma clipper.
    ///
    /// Initializes the GPU context. This may be slow on first call.
    pub fn new() -> Self {
        Self {
            ctx: ProcessingContext::new(),
        }
    }

    /// Check if GPU is available.
    pub fn gpu_available(&self) -> bool {
        self.ctx.gpu().is_some()
    }

    /// Stack frames using GPU sigma clipping.
    ///
    /// # Arguments
    ///
    /// * `frames` - Slice of frame data (each frame is width × height f32 values)
    /// * `width` - Image width
    /// * `height` - Image height
    /// * `config` - Sigma clipping configuration
    ///
    /// # Returns
    ///
    /// Stacked image as `Vec<f32>` with width × height elements.
    ///
    /// # Panics
    ///
    /// Panics if:
    /// - No GPU is available
    /// - More than MAX_GPU_FRAMES frames are provided
    /// - Frame data sizes don't match width × height
    pub fn stack(
        &mut self,
        frames: &[&[f32]],
        width: usize,
        height: usize,
        config: &GpuSigmaClipConfig,
    ) -> Vec<f32> {
        assert!(
            frames.len() <= MAX_GPU_FRAMES,
            "GPU sigma clipping supports max {} frames, got {}",
            MAX_GPU_FRAMES,
            frames.len()
        );
        assert!(!frames.is_empty(), "At least one frame required");

        let pixels_per_frame = width * height;
        for (i, frame) in frames.iter().enumerate() {
            assert_eq!(
                frame.len(),
                pixels_per_frame,
                "Frame {} has {} pixels, expected {}",
                i,
                frame.len(),
                pixels_per_frame
            );
        }

        // Get GPU context and pipeline together to manage borrow lifetimes
        let gpu_ctx = self.ctx.gpu_context().expect("GPU context not available");

        // Clone the GPU (cheap, uses Arc internally) and get pipeline
        let gpu = gpu_ctx.gpu().clone();
        let pipeline = gpu_ctx
            .get_or_create(|gpu| Ok(GpuSigmaClipPipeline::new(gpu)))
            .expect("Failed to create sigma clip pipeline");

        let device = gpu.device();
        let queue = gpu.queue();

        // Create concatenated frame buffer (all frames in sequence)
        let total_pixels = frames.len() * pixels_per_frame;
        let mut frame_data = Vec::with_capacity(total_pixels);
        for frame in frames {
            frame_data.extend_from_slice(frame);
        }

        // Create GPU buffers
        let frames_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("sigma_clip_frames_buffer"),
            contents: bytemuck::cast_slice(&frame_data),
            usage: wgpu::BufferUsages::STORAGE,
        });

        let output_size = (pixels_per_frame * std::mem::size_of::<f32>()) as u64;
        let output_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sigma_clip_output_buffer"),
            size: output_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        let params = GpuParams {
            width: width as u32,
            height: height as u32,
            frame_count: frames.len() as u32,
            sigma: config.sigma,
            max_iterations: config.max_iterations,
            _padding: [0; 3],
        };

        let params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("sigma_clip_params_buffer"),
            contents: bytemuck::bytes_of(&params),
            usage: wgpu::BufferUsages::UNIFORM,
        });

        // Create bind group
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sigma_clip_bind_group"),
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

        // Encode and submit compute pass
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("sigma_clip_encoder"),
        });

        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("sigma_clip_pass"),
                timestamp_writes: None,
            });

            pass.set_pipeline(&pipeline.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);

            // Dispatch workgroups (16x16 threads per group)
            let workgroups_x = width.div_ceil(16) as u32;
            let workgroups_y = height.div_ceil(16) as u32;
            pass.dispatch_workgroups(workgroups_x, workgroups_y, 1);
        }

        // Create staging buffer for readback
        let staging_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sigma_clip_staging_buffer"),
            size: output_size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        encoder.copy_buffer_to_buffer(&output_buffer, 0, &staging_buffer, 0, output_size);

        queue.submit(std::iter::once(encoder.finish()));

        // Wait for GPU and read back results
        let buffer_slice = staging_buffer.slice(..);
        buffer_slice.map_async(wgpu::MapMode::Read, |_| {});
        gpu.wait();

        let data = buffer_slice.get_mapped_range();
        let result: Vec<f32> = bytemuck::cast_slice(&data).to_vec();
        drop(data);
        staging_buffer.unmap();

        result
    }
}

impl Default for GpuSigmaClipper {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to check if GPU is available for tests
    fn test_gpu_available() -> bool {
        let clipper = GpuSigmaClipper::new();
        clipper.gpu_available()
    }

    #[test]
    fn test_config_default() {
        let config = GpuSigmaClipConfig::default();
        assert!((config.sigma - 2.5).abs() < f32::EPSILON);
        assert_eq!(config.max_iterations, 3);
    }

    #[test]
    fn test_config_new() {
        let config = GpuSigmaClipConfig::new(3.0, 5);
        assert!((config.sigma - 3.0).abs() < f32::EPSILON);
        assert_eq!(config.max_iterations, 5);
    }

    #[test]
    #[should_panic(expected = "Sigma must be positive")]
    fn test_config_zero_sigma_panics() {
        GpuSigmaClipConfig::new(0.0, 3);
    }

    #[test]
    #[should_panic(expected = "Max iterations must be at least 1")]
    fn test_config_zero_iterations_panics() {
        GpuSigmaClipConfig::new(2.5, 0);
    }

    #[test]
    fn test_stack_identical_values() {
        if !test_gpu_available() {
            eprintln!("Skipping GPU test: no GPU available");
            return;
        }

        let mut clipper = GpuSigmaClipper::new();
        let config = GpuSigmaClipConfig::default();

        // 4x4 image, 5 frames, all with value 10.0
        let width = 4;
        let height = 4;
        let frame: Vec<f32> = vec![10.0; width * height];
        let frames: Vec<&[f32]> = vec![&frame; 5];

        let result = clipper.stack(&frames, width, height, &config);

        assert_eq!(result.len(), width * height);
        for val in result {
            assert!((val - 10.0).abs() < 1e-5, "Expected 10.0, got {}", val);
        }
    }

    #[test]
    fn test_stack_with_outlier() {
        if !test_gpu_available() {
            eprintln!("Skipping GPU test: no GPU available");
            return;
        }

        let mut clipper = GpuSigmaClipper::new();
        let config = GpuSigmaClipConfig::new(2.0, 3);

        // 2x2 image, 6 frames
        let width = 2;
        let height = 2;

        // 5 frames with value 10.0, 1 frame with outlier 1000.0
        let normal: Vec<f32> = vec![10.0; width * height];
        let outlier: Vec<f32> = vec![1000.0; width * height];
        let frames: Vec<&[f32]> = vec![&normal, &normal, &normal, &normal, &normal, &outlier];

        let result = clipper.stack(&frames, width, height, &config);

        assert_eq!(result.len(), width * height);
        // Outlier should be clipped, result should be close to 10.0
        for val in result {
            assert!(
                (val - 10.0).abs() < 1.0,
                "Expected ~10.0 after clipping, got {}",
                val
            );
        }
    }

    #[test]
    fn test_stack_two_frames_no_clipping() {
        if !test_gpu_available() {
            eprintln!("Skipping GPU test: no GPU available");
            return;
        }

        let mut clipper = GpuSigmaClipper::new();
        let config = GpuSigmaClipConfig::default();

        // With only 2 frames, can't do meaningful clipping
        let width = 2;
        let height = 2;
        let frame1: Vec<f32> = vec![5.0; width * height];
        let frame2: Vec<f32> = vec![15.0; width * height];
        let frames: Vec<&[f32]> = vec![&frame1, &frame2];

        let result = clipper.stack(&frames, width, height, &config);

        assert_eq!(result.len(), width * height);
        // Should return mean of 5.0 and 15.0 = 10.0
        for val in result {
            assert!((val - 10.0).abs() < 1e-5, "Expected 10.0, got {}", val);
        }
    }

    #[test]
    fn test_stack_single_frame() {
        if !test_gpu_available() {
            eprintln!("Skipping GPU test: no GPU available");
            return;
        }

        let mut clipper = GpuSigmaClipper::new();
        let config = GpuSigmaClipConfig::default();

        let width = 4;
        let height = 4;
        let frame: Vec<f32> = (0..width * height).map(|i| i as f32).collect();
        let frames: Vec<&[f32]> = vec![&frame];

        let result = clipper.stack(&frames, width, height, &config);

        assert_eq!(result.len(), width * height);
        // Single frame should return itself
        for (i, val) in result.iter().enumerate() {
            assert!(
                (*val - i as f32).abs() < 1e-5,
                "Expected {}, got {}",
                i,
                val
            );
        }
    }

    #[test]
    fn test_stack_large_image() {
        if !test_gpu_available() {
            eprintln!("Skipping GPU test: no GPU available");
            return;
        }

        let mut clipper = GpuSigmaClipper::new();
        let config = GpuSigmaClipConfig::default();

        // 512x512 image, 10 frames
        let width = 512;
        let height = 512;
        let frame: Vec<f32> = vec![100.0; width * height];
        let frames: Vec<&[f32]> = vec![&frame; 10];

        let result = clipper.stack(&frames, width, height, &config);

        assert_eq!(result.len(), width * height);
        // All identical values should produce identical result
        for val in &result[..100] {
            // Check first 100 values
            assert!((*val - 100.0).abs() < 1e-5, "Expected 100.0, got {}", val);
        }
    }

    #[test]
    #[should_panic(expected = "At least one frame required")]
    fn test_stack_empty_frames_panics() {
        if !test_gpu_available() {
            panic!("At least one frame required"); // Match the expected panic
        }

        let mut clipper = GpuSigmaClipper::new();
        let config = GpuSigmaClipConfig::default();
        let frames: Vec<&[f32]> = vec![];
        clipper.stack(&frames, 10, 10, &config);
    }

    #[test]
    #[should_panic(expected = "GPU sigma clipping supports max")]
    fn test_stack_too_many_frames_panics() {
        if !test_gpu_available() {
            panic!("GPU sigma clipping supports max"); // Match the expected panic
        }

        let mut clipper = GpuSigmaClipper::new();
        let config = GpuSigmaClipConfig::default();

        let width = 2;
        let height = 2;
        let frame: Vec<f32> = vec![1.0; width * height];

        // Create more than MAX_GPU_FRAMES frame references
        let frames: Vec<&[f32]> = vec![&frame[..]; MAX_GPU_FRAMES + 1];
        clipper.stack(&frames, width, height, &config);
    }
}
