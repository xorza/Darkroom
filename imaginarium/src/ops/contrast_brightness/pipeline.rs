use crate::common::error::Result;
use crate::gpu::Gpu;
use crate::processing_context::GpuPipeline;

const CONTRAST_BRIGHTNESS_SHADER: &str = include_str!("contrast_brightness.wgsl");

/// Cached GPU pipeline for contrast/brightness operations.
/// Create once and reuse for multiple executions.
#[derive(Debug)]
pub struct GpuContrastBrightnessPipeline {
    pub(super) compute_pipeline: wgpu::ComputePipeline,
    pub(super) bind_group_layout: wgpu::BindGroupLayout,
}

impl GpuContrastBrightnessPipeline {
    /// Creates a new cached pipeline for contrast/brightness operations.
    pub fn new(ctx: &Gpu) -> Result<Self> {
        let device = ctx.device();

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("contrast_brightness_shader"),
            source: wgpu::ShaderSource::Wgsl(CONTRAST_BRIGHTNESS_SHADER.into()),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("contrast_brightness_bind_group_layout"),
            entries: &[
                // Params uniform
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // Input image
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // Output image
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("contrast_brightness_pipeline_layout"),
            bind_group_layouts: &[&bind_group_layout],
            immediate_size: 0,
        });

        let compute_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("contrast_brightness_pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });

        Ok(Self {
            compute_pipeline,
            bind_group_layout,
        })
    }
}

impl GpuPipeline for GpuContrastBrightnessPipeline {}
