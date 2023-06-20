use std::borrow::Cow;
use std::f32::consts;
use std::mem;

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, UVec2, Vec3};
use wgpu::*;
use wgpu::util::DeviceExt;

use crate::app_base::RenderInfo;
use crate::view::View;

fn vertex(pos: [f32; 3], tc: [f32; 2]) -> Vertex {
    Vertex {
        _pos: [pos[0], pos[1], pos[2], 1.0],
        _tex_coord: [tc[0], tc[1]],
    }
}

fn create_vertices() -> Vec<Vertex> {
    // @formatter:off
    let vertex_data = [
        vertex([   0.0,   0.0, 0.0], [ 0.0, 0.0]),
        vertex([ 200.0,   0.0, 0.0], [ 1.0, 0.0]),
        vertex([ 200.0, 200.0, 0.0], [ 1.0, 1.0]),

        vertex([   0.0,   0.0, 0.0], [ 0.0, 0.0]),
        vertex([ 200.0, 200.0, 0.0], [ 1.0, 1.0]),
        vertex([   0.0, 200.0, 0.0], [ 0.0, 1.0]),
    ];
    // @formatter:on

    vertex_data.to_vec()
}

fn create_texels(size: usize) -> Vec<u8> {
    (0..size * size)
        .map(|id| {
            let cx = 3.0 * (id % size) as f32 / (size - 1) as f32 - 2.0;
            let cy = 2.0 * (id / size) as f32 / (size - 1) as f32 - 1.0;
            let (mut x, mut y, mut count) = (cx, cy, 0);
            while count < 0xFF && x * x + y * y < 4.0 {
                let old_x = x;
                x = x * x - y * y + cx;
                y = 2.0 * old_x * y + cy;
                count += 1;
            }
            count
        })
        .collect()
}


pub(crate) struct RenderCache {
    vertex_buf: Buffer,
    vertex_count: u32,
    bind_group: BindGroup,
    uniform_buf: Buffer,
    pipeline: RenderPipeline,
    id_texture: Texture,
}


pub trait Renderer {
    fn background(&self);
}

pub(crate) struct WgpuRenderer<'a> {
    cache: &'a RenderCache,
    window_size: UVec2,
}

impl Renderer for WgpuRenderer<'_> {
    fn background(&self) {}
}


#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    _pos: [f32; 4],
    _tex_coord: [f32; 2],
}

impl RenderCache {
    pub fn new(device: &Device,
               queue: &Queue,
               surface_config: &SurfaceConfiguration,
               window_size: UVec2) -> Self {
        let vertex_size = mem::size_of::<Vertex>();
        let vertex_data = create_vertices();

        let vertex_buf = device.create_buffer_init(&util::BufferInitDescriptor {
            label: Some("Cube Vertex Buffer"),
            contents: bytemuck::cast_slice(&vertex_data),
            usage: BufferUsages::VERTEX,
        });

        let bind_group_layout = device.create_bind_group_layout(
            &BindGroupLayoutDescriptor {
                label: None,
                entries: &[
                    BindGroupLayoutEntry {
                        binding: 0,
                        visibility: ShaderStages::VERTEX,
                        ty: BindingType::Buffer {
                            ty: BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: BufferSize::new(64),
                        },
                        count: None,
                    },
                    BindGroupLayoutEntry {
                        binding: 1,
                        visibility: ShaderStages::FRAGMENT,
                        ty: BindingType::Texture {
                            multisampled: false,
                            sample_type: TextureSampleType::Uint,
                            view_dimension: TextureViewDimension::D2,
                        },
                        count: None,
                    },
                ],
            });
        let pipeline_layout = device.create_pipeline_layout(
            &PipelineLayoutDescriptor {
                label: None,
                bind_group_layouts: &[&bind_group_layout],
                push_constant_ranges: &[],
            });

        let size = 256u32;
        let texels = create_texels(size as usize);
        let texture_extent = Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        };
        let texture = device.create_texture(&TextureDescriptor {
            label: Some("Mandelbrot Set Texture"),
            size: texture_extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: TextureFormat::R8Uint,
            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let texture_view = texture.create_view(&TextureViewDescriptor::default());
        queue.write_texture(
            texture.as_image_copy(),
            &texels,
            ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(size),
                rows_per_image: None,
            },
            texture_extent,
        );

        let uniform_buf = device.create_buffer(&BufferDescriptor {
            label: Some("Uniform Buffer"),
            size: 64,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group = device.create_bind_group(&BindGroupDescriptor {
            layout: &bind_group_layout,
            entries: &[
                BindGroupEntry {
                    binding: 0,
                    resource: uniform_buf.as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::TextureView(&texture_view),
                },
            ],
            label: None,
        });

        let shader = device.create_shader_module(ShaderModuleDescriptor {
            label: None,
            source: ShaderSource::Wgsl(Cow::Borrowed(include_str!("shader.wgsl"))),
        });

        let vertex_buffers = [VertexBufferLayout {
            array_stride: vertex_size as BufferAddress,
            step_mode: VertexStepMode::Vertex,
            attributes: &[
                VertexAttribute {
                    format: VertexFormat::Float32x4,
                    offset: 0,
                    shader_location: 0,
                },
                VertexAttribute {
                    format: VertexFormat::Float32x2,
                    offset: 4 * 4,
                    shader_location: 1,
                },
            ],
        }];

        let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
            label: None,
            layout: Some(&pipeline_layout),
            vertex: VertexState {
                module: &shader,
                entry_point: "vs_main",
                buffers: &vertex_buffers,
            },
            fragment: Some(FragmentState {
                module: &shader,
                entry_point: "fs_main",
                targets: &[Some(surface_config.view_formats[0].into())],
            }),
            primitive: PrimitiveState {
                cull_mode: Some(Face::Back),
                front_face: FrontFace::Cw,

                ..Default::default()
            },
            depth_stencil: None,
            multisample: MultisampleState::default(),
            multiview: None,
        });

        let id_texture = Self::create_id_texture(device, window_size);


        RenderCache {
            vertex_buf,
            vertex_count: vertex_data.len() as u32,
            bind_group,
            uniform_buf,
            pipeline,
            id_texture,
        }
    }

    pub(crate) fn resize(&mut self, device: &Device, _queue: &Queue, window_size: UVec2) {
        if self.id_texture.width() != window_size.x || self.id_texture.height() != window_size.y {
            self.id_texture = Self::create_id_texture(device, window_size);
        }
    }

    fn create_id_texture(device: &Device, window_size: UVec2) -> Texture {
        let id_texture = device.create_texture(&TextureDescriptor {
            label: Some("Id Texture"),
            size: Extent3d {
                width: window_size.x,
                height: window_size.y,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: TextureFormat::R32Uint,
            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST | TextureUsages::COPY_SRC,
            view_formats: &[],
        });

        id_texture
    }
}

impl<'a> WgpuRenderer<'a> {
    pub fn new(render_cache: &'a RenderCache, window_size: UVec2) -> Self {
        Self {
            cache: render_cache,
            window_size,
        }
    }

    pub fn begin_frame(&self, render: &RenderInfo) {
        let size = self.window_size;

        let projection = Mat4::orthographic_lh(
            0.0,
            size.x as f32,
            size.y as f32,
            0.0,
            -1.0,
            1.0,
        );

        render.queue.write_buffer(
            &self.cache.uniform_buf,
            0,
            bytemuck::cast_slice(projection.as_ref()),
        );
    }

    pub fn render_view(&self, render: &RenderInfo, _view: &dyn View) {
        let mut encoder =
            render.device.create_command_encoder(&CommandEncoderDescriptor { label: None });

        {
            let mut render_pass = encoder.begin_render_pass(
                &RenderPassDescriptor {
                    label: None,
                    color_attachments: &[Some(RenderPassColorAttachment {
                        view: render.view,
                        resolve_target: None,
                        ops: Operations {
                            load: LoadOp::Clear(Color::RED),
                            store: true,
                        },
                    })],
                    depth_stencil_attachment: None,
                });
            render_pass.push_debug_group("Prepare data for draw.");
            render_pass.set_pipeline(&self.cache.pipeline);
            render_pass.set_bind_group(0, &self.cache.bind_group, &[]);
            render_pass.set_vertex_buffer(0, self.cache.vertex_buf.slice(..));
            render_pass.pop_debug_group();
            render_pass.insert_debug_marker("Draw!");
            render_pass.draw(0..self.cache.vertex_count, 0..1);
        }

        render.queue.submit(Some(encoder.finish()));
    }
}
