use crate::common::error::Result;
use crate::gpu::Gpu;
use crate::processing_context::GpuPipeline;

const BLEND_SHADER: &str = include_str!("blend.wgsl");

/// Cached GPU pipeline for blend operations.
/// Create once and reuse for multiple executions.
#[derive(Debug)]
pub struct GpuBlendPipeline {
    pub(super) compute_pipeline: wgpu::ComputePipeline,
    pub(super) bind_group_layout: wgpu::BindGroupLayout,
}

impl GpuBlendPipeline {
    /// Creates a new cached pipeline for blend operations.
    pub fn new(ctx: &Gpu) -> Result<Self> {
        let device = ctx.device();

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("blend_shader"),
            source: wgpu::ShaderSource::Wgsl(BLEND_SHADER.into()),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("blend_bind_group_layout"),
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
                // Source image (top layer)
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
                // Destination image (bottom layer)
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
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
                    binding: 3,
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
            label: Some("blend_pipeline_layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let compute_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("blend_pipeline"),
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

impl GpuPipeline for GpuBlendPipeline {}
