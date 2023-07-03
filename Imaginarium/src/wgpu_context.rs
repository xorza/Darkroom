use bytemuck::{Pod, Zeroable};
use pollster::FutureExt;
use wgpu::util::DeviceExt;

pub(crate) struct WgpuContext {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub one_vertex_buffer: wgpu::Buffer,
}

fn aligned_size_of_uniform<U: Sized>() -> wgpu::BufferAddress {
    let uniform_size = std::mem::size_of::<U>();
    let uniform_align = 256;
    let uniform_padded_size = (uniform_size + uniform_align - 1) / uniform_align * uniform_align;

    uniform_padded_size as wgpu::BufferAddress
}

impl WgpuContext {
    pub fn new() -> anyhow::Result<WgpuContext> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY,
            dx12_shader_compiler: wgpu::Dx12Compiler::Dxc { dxil_path: None, dxc_path: None },
        });
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: None,
            })
            .block_on()
            .expect("Unable to find a suitable GPU adapter.");

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default(), None)
            .block_on()
            .expect("Unable to find a suitable GPU device.");

        let rect = Rect::one();

        let one_vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            contents: rect.as_bytes(),
            usage: wgpu::BufferUsages::VERTEX,
            label: Some("Vertex Buffer"),
        });

        Ok(WgpuContext {
            device,
            queue,
            one_vertex_buffer,
        })
    }

    pub fn draw_one(
        &self,
        encoder:&mut wgpu::CommandEncoder,
        tex_view: &wgpu::TextureView,
        target_tex_view: &wgpu::TextureView,
        shader: &wgpu::ShaderModule,
    ) {
        let device = &self.device;

        let bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            multisampled: false,
                            sample_type: wgpu::TextureSampleType::Float { filterable: false },
                            view_dimension: wgpu::TextureViewDimension::D2,
                        },
                        count: None,
                    },
                ],
                label: None,
            });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(tex_view),
                },
            ],
            label: None,
        });


        let pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                bind_group_layouts: &[&bind_group_layout],
                push_constant_ranges: &[],
                label: None,
            });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_main",
                buffers: &[Rect::desc()],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::One,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::One,
                            operation: wgpu::BlendOperation::Max,
                        },
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            label: None,
        });


        {
            let mut render_pass = encoder.begin_render_pass(
                &wgpu::RenderPassDescriptor {
                    color_attachments: &[
                        Some(wgpu::RenderPassColorAttachment {
                            view: target_tex_view,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Load,
                                store: true,
                            },
                        }),
                    ],
                    depth_stencil_attachment: None,
                    label: None,
                });

            render_pass.push_debug_group("Prepare data for draw.");

            render_pass.set_pipeline(&pipeline);
            render_pass.set_bind_group(0, &bind_group, &[]);

            render_pass.insert_debug_marker("Draw.");
            render_pass.set_vertex_buffer(0, self.one_vertex_buffer.slice(..));
            render_pass.draw(0..Rect::vert_count(), 0..1);

            render_pass.pop_debug_group();
        }
    }
}


#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub(crate) struct Vert(pub [f32; 2], pub [f32; 2]);

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub(crate) struct Rect(pub [Vert; 4]);

impl Rect {
    pub fn one() -> Rect {
        Rect([
            Vert([0.0, 0.0], [0.0, 0.0]),
            Vert([1.0, 0.0], [1.0, 0.0]),
            Vert([0.0, 1.0], [0.0, 1.0]),
            Vert([1.0, 1.0], [1.0, 1.0]),
        ])
    }

    pub fn new(width: f32, height: f32, tex_width: f32, tex_height: f32) -> Rect {
        Rect([
            Vert([0.0, 0.0], [0.0, 0.0]),
            Vert([width, 0.0], [tex_width, 0.0]),
            Vert([0.0, height], [0.0, tex_height]),
            Vert([width, height], [tex_width, tex_height]),
        ])
    }

    pub fn desc<'a>() -> wgpu::VertexBufferLayout<'a> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vert>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute {
                    offset: 0,
                    format: wgpu::VertexFormat::Float32x2,
                    shader_location: 0,
                },
                wgpu::VertexAttribute {
                    offset: std::mem::size_of::<[f32; 2]>() as wgpu::BufferAddress,
                    format: wgpu::VertexFormat::Float32x2,
                    shader_location: 1,
                },
            ],
        }
    }

    pub fn as_bytes(&self) -> &[u8] {
        bytemuck::cast_slice(&self.0)
    }
    pub fn size_in_bytes() -> u32 {
        std::mem::size_of::<Rect>() as u32
    }
    pub fn vert_count() -> u32 {
        4u32
    }
    pub fn stride(&self) -> u32 {
        std::mem::size_of::<Vert>() as u32
    }
}
