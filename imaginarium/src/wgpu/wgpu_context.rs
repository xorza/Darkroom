use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::RangeBounds;
use std::rc::Rc;
use std::thread;

use bytemuck::Pod;
use pollster::FutureExt;
use wgpu::util::DeviceExt;

use crate::color_format::ColorFormat;
use crate::image::{Image, ImageDesc};
use crate::math::{Transform2D, Vert2D};

fn aligned_size_of_uniform<U: Sized>() -> u64 {
    let uniform_size = std::mem::size_of::<U>();
    const UNIFORM_ALIGN: usize = 256;
    let uniform_padded_size = (uniform_size + UNIFORM_ALIGN - 1) / UNIFORM_ALIGN * UNIFORM_ALIGN;

    uniform_padded_size as u64
}

pub enum Action<'a> {
    RunShader {
        shader: &'a Shader,
        shader_entry_name: &'a str,
        input_textures: Vec<&'a TextureWithTransform>,
        output_texture: &'a Texture,
        fragment_push_constant: &'a [u8],
    },
    ImgToTex(Vec<(&'a Image, &'a Texture)>),
    TexToImg(Vec<(&'a Texture, RefCell<&'a mut Image>)>),
}

pub struct WgpuContext {
    device: wgpu::Device,
    queue: wgpu::Queue,
    limits: wgpu::Limits,
    rect_one_vb: VertexBuffer,
    default_sampler: wgpu::Sampler,
    encoder: RefCell<Option<wgpu::CommandEncoder>>,
    common_vertex_shader_module: wgpu::ShaderModule,
}

impl WgpuContext {
    pub fn new() -> anyhow::Result<WgpuContext> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY,
            flags: Default::default(),
            dx12_shader_compiler: wgpu::Dx12Compiler::Dxc {
                dxil_path: None,
                dxc_path: None,
            },
            gles_minor_version: Default::default(),
        });
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: None,
            })
            .block_on()
            .expect("Unable to find a suitable GPU adapter.");

        let _limits = adapter.limits();
        let limits = wgpu::Limits {
            max_push_constant_size: 256,
            max_texture_dimension_1d: 16384,
            max_texture_dimension_2d: 16384,
            ..Default::default()
        };

        let device_descriptor = wgpu::DeviceDescriptor {
            label: None,
            required_features: wgpu::Features::PUSH_CONSTANTS
                | wgpu::Features::ADDRESS_MODE_CLAMP_TO_BORDER,
            required_limits: limits.clone(),
        };

        let (device, queue) = adapter
            .request_device(&device_descriptor, None)
            .block_on()
            .expect("Unable to find a suitable GPU device.");

        let rect_one_vb = VertexBuffer::from_slice(&device, &Vert2D::rect_one());

        let default_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::ClampToBorder,
            address_mode_v: wgpu::AddressMode::ClampToBorder,
            border_color: Some(wgpu::SamplerBorderColor::TransparentBlack),
            ..Default::default()
        });

        let common_vertex_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None,
            source: wgpu::ShaderSource::Wgsl(include_str!("common_vert.wgsl").into()),
        });

        Ok(WgpuContext {
            device,
            queue,
            limits,
            rect_one_vb,
            default_sampler,
            encoder: RefCell::new(None),
            common_vertex_shader_module: common_vertex_shader,
        })
    }

    pub fn perform(&self, actions: &[Action]) {
        let mut buffer_images: Option<Vec<BufferImage>> = None;

        for (action_index, action) in actions.iter().enumerate() {
            match action {
                Action::RunShader {
                    shader,
                    shader_entry_name,
                    input_textures,
                    output_texture,
                    fragment_push_constant,
                } => {
                    let mut encoder_temp = self.encoder.borrow_mut();
                    let encoder = encoder_temp.get_or_insert_with(|| {
                        self.device
                            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None })
                    });

                    let mut push_constant = input_textures
                        .iter()
                        .flat_map(|t| bytemuck::bytes_of(&t.transform).to_vec())
                        .collect::<Vec<u8>>();
                    push_constant.extend_from_slice(fragment_push_constant);

                    self.run_shader(
                        encoder,
                        shader,
                        shader_entry_name,
                        input_textures,
                        output_texture,
                        push_constant.as_slice(),
                    );
                }

                Action::ImgToTex(img_tex) => {
                    for (image, texture) in img_tex.iter() {
                        if image.desc != texture.desc {
                            panic!("Image and texture must have the same dimensions");
                        }
                        let desc = &image.desc;

                        self.queue.write_texture(
                            texture.texture.as_image_copy(),
                            &image.bytes,
                            wgpu::ImageDataLayout {
                                offset: 0,
                                bytes_per_row: Some(desc.stride()),
                                rows_per_image: Some(desc.height()),
                            },
                            texture.extent,
                        );
                    }
                }

                Action::TexToImg(tex_img) => {
                    let mut encoder_temp = self.encoder.borrow_mut();
                    let encoder = encoder_temp.get_or_insert_with(|| {
                        self.device
                            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None })
                    });

                    for (index_in_action, (texture, image)) in tex_img.iter().enumerate() {
                        let image = image.borrow();

                        if image.desc != texture.desc {
                            panic!("Image and texture must have the same dimensions");
                        }
                        let desc = &image.desc;

                        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                            size: desc.size_in_bytes() as wgpu::BufferAddress,
                            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                            mapped_at_creation: false,
                            label: None,
                        });

                        encoder.copy_texture_to_buffer(
                            wgpu::ImageCopyTexture {
                                texture: &texture.texture,
                                mip_level: 0,
                                origin: wgpu::Origin3d::ZERO,
                                aspect: Default::default(),
                            },
                            wgpu::ImageCopyBuffer {
                                buffer: &buffer,
                                layout: wgpu::ImageDataLayout {
                                    offset: 0,
                                    bytes_per_row: Some(desc.stride()),
                                    rows_per_image: Some(desc.height()),
                                },
                            },
                            texture.extent,
                        );

                        buffer_images
                            .get_or_insert_with(Vec::new)
                            .push(BufferImage {
                                buffer,
                                image_index: (action_index, index_in_action),
                            });
                    }
                }
            }
        }

        if let Some(buffer_images) = buffer_images {
            self.sync();

            let slices = buffer_images
                .iter()
                .map(|buf_img| {
                    let slice = buf_img.buffer.slice(..);
                    slice.map_async(wgpu::MapMode::Read, |result| {
                        result.unwrap();
                    });
                    slice
                })
                .collect::<Vec<wgpu::BufferSlice>>();

            self.device.poll(wgpu::Maintain::Wait);

            for (slice_index, buf_img) in buffer_images.iter().enumerate() {
                let (action_index, index_in_action) = buf_img.image_index;
                if let Action::TexToImg(tex_to_img) = &actions[action_index] {
                    let mut image = tex_to_img[index_in_action].1.borrow_mut();

                    let data = slices[slice_index].get_mapped_range();
                    image.bytes = data.to_vec();
                    drop(data);

                    buf_img.buffer.unmap();
                } else {
                    panic!("Expected TexToImg action.");
                }
            }
        }
    }

    pub fn sync(&self) {
        if let Some(encoder) = self.encoder.replace(None) {
            self.queue.submit(Some(encoder.finish()));
            self.device.poll(wgpu::Maintain::Wait);
        }
    }

    pub fn create_shader(
        &self,
        shader: &str,
        input_texture_count: u32,
        push_constant_size: u32,
    ) -> Shader {
        Shader::new(
            &self.device,
            shader,
            input_texture_count,
            push_constant_size,
        )
    }

    pub(crate) fn create_texture(&self, image_desc: ImageDesc) -> Texture {
        let extent = wgpu::Extent3d {
            width: image_desc.width(),
            height: image_desc.height(),
            depth_or_array_layers: 1,
        };

        let usage = wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::COPY_SRC;

        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::from(&image_desc.color_format()),
            usage,
            view_formats: &[],
        });

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        Texture {
            desc: image_desc,
            texture,
            view,
            extent,
        }
    }

    fn run_shader(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        shader: &Shader,
        shader_entry_name: &str,
        input_textures: &[&TextureWithTransform],
        output_texture: &Texture,
        push_constant: &[u8],
    ) {
        assert_eq!(input_textures.len() as u32, shader.input_texture_count);
        assert_eq!(
            shader.fragment_push_constant_size + shader.vertex_push_constant_size,
            push_constant.len() as u32
        );

        let device = &self.device;

        let mut bind_entries: Vec<wgpu::BindGroupEntry> = Vec::new();
        bind_entries.push(wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::Sampler(&self.default_sampler),
        });
        input_textures.iter().enumerate().for_each(|(index, tex)| {
            bind_entries.push(wgpu::BindGroupEntry {
                binding: index as u32 + 1,
                resource: wgpu::BindingResource::TextureView(&tex.texture.view),
            });
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &shader.bind_group_layout,
            entries: bind_entries.as_slice(),
            label: None,
        });
        let pipeline = shader.get_pipeline(
            device,
            &self.common_vertex_shader_module,
            shader_entry_name,
            &output_texture.desc.color_format(),
        );

        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &output_texture.view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::GREEN),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                label: None,
                occlusion_query_set: None,
            });

            render_pass.push_debug_group("Prepare data for draw.");

            render_pass.set_pipeline(&pipeline);
            render_pass.set_bind_group(0, &bind_group, &[]);
            render_pass.set_push_constants(wgpu::ShaderStages::VERTEX, 0, push_constant);

            render_pass.pop_debug_group();

            render_pass.insert_debug_marker("Draw.");
            render_pass.set_vertex_buffer(0, self.rect_one_vb.slice(..));
            render_pass.draw(0..self.rect_one_vb.vert_count, 0..1);
        }
    }
}

impl Drop for WgpuContext {
    fn drop(&mut self) {
        if self.encoder.borrow().is_some() && !thread::panicking() {
            panic!("WgpuContext dropped before encoder was submitted. Try calling WgpuContext::sync().");
        }
    }
}

pub(crate) struct VertexBuffer {
    pub(crate) buffer: wgpu::Buffer,
    pub(crate) vert_count: u32,
    pub(crate) stride: u32,
}

impl VertexBuffer {
    pub fn from_vec<V: Pod>(device: &wgpu::Device, data: Vec<V>) -> Self {
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            contents: bytemuck::cast_slice(&data),
            usage: wgpu::BufferUsages::VERTEX,
            label: None,
        });

        VertexBuffer {
            buffer,
            vert_count: data.len() as u32,
            stride: std::mem::size_of::<V>() as u32,
        }
    }
    pub fn from_slice<V: Pod>(device: &wgpu::Device, data: &[V]) -> Self {
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            contents: bytemuck::cast_slice(data),
            usage: wgpu::BufferUsages::VERTEX,
            label: None,
        });

        VertexBuffer {
            buffer,
            vert_count: data.len() as u32,
            stride: std::mem::size_of::<V>() as u32,
        }
    }
    pub fn slice<S: RangeBounds<u64>>(&self, range: S) -> wgpu::BufferSlice {
        self.buffer.slice(range)
    }
}

pub struct Shader {
    pub(crate) module: wgpu::ShaderModule,
    bind_group_layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    input_texture_count: u32,
    vertex_push_constant_size: u32,
    fragment_push_constant_size: u32,
    vertex_layout: Vec<wgpu::VertexFormat>,
    vertex_stride: u64,
    vertex_attributes: Vec<wgpu::VertexAttribute>,
    pipeline_cache: RefCell<HashMap<(String, ColorFormat), Rc<wgpu::RenderPipeline>>>,
}

impl Shader {
    pub(crate) fn new(
        device: &wgpu::Device,
        shader: &str,
        input_texture_count: u32,
        fragment_push_constant_size: u32,
    ) -> Shader {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None,
            source: wgpu::ShaderSource::Wgsl(shader.into()),
        });

        let mut wgpu_bind_group_layout_entries: Vec<wgpu::BindGroupLayoutEntry> = Vec::new();
        wgpu_bind_group_layout_entries.push(wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
            count: None,
        });
        wgpu_bind_group_layout_entries.extend((0..input_texture_count as usize).map(|index| {
            wgpu::BindGroupLayoutEntry {
                binding: index as u32 + 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    multisampled: false,
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                },
                count: None,
            }
        }));

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            entries: &wgpu_bind_group_layout_entries,
            label: None,
        });

        let vertex_push_constant_size =
            input_texture_count * std::mem::size_of::<Transform2D>() as u32;

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[
                wgpu::PushConstantRange {
                    stages: wgpu::ShaderStages::VERTEX,
                    range: 0..vertex_push_constant_size,
                },
                wgpu::PushConstantRange {
                    stages: wgpu::ShaderStages::FRAGMENT,
                    range: vertex_push_constant_size
                        ..fragment_push_constant_size + vertex_push_constant_size,
                },
            ],
            label: None,
        });

        let vertex_layout = vec![wgpu::VertexFormat::Float32x2, wgpu::VertexFormat::Float32x2];
        let mut vertex_stride: u64 = 0;
        let mut vertex_attributes: Vec<wgpu::VertexAttribute> = Vec::new();
        for (index, entry) in vertex_layout.iter().enumerate() {
            vertex_attributes.push(wgpu::VertexAttribute {
                offset: vertex_stride,
                format: *entry,
                shader_location: index as u32,
            });
            vertex_stride += entry.size();
        }

        Shader {
            module,
            bind_group_layout,
            pipeline_layout,
            input_texture_count,
            vertex_push_constant_size,
            fragment_push_constant_size,
            vertex_layout,
            vertex_stride,
            vertex_attributes,
            pipeline_cache: RefCell::default(),
        }
    }

    fn get_pipeline(
        &self,
        device: &wgpu::Device,
        vertex_shader: &wgpu::ShaderModule,
        shader_entry_name: &str,
        color_format: &ColorFormat,
    ) -> Rc<wgpu::RenderPipeline> {
        self.pipeline_cache
            .borrow_mut()
            .entry((shader_entry_name.to_string(), *color_format))
            .or_insert_with(|| {
                Rc::from(
                    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                        layout: Some(&self.pipeline_layout),
                        vertex: wgpu::VertexState {
                            module: vertex_shader,
                            entry_point: "vs_main",
                            compilation_options: Default::default(),
                            buffers: &[wgpu::VertexBufferLayout {
                                array_stride: self.vertex_stride,
                                step_mode: wgpu::VertexStepMode::Vertex,
                                attributes: self.vertex_attributes.as_slice(),
                            }],
                        },
                        fragment: Some(wgpu::FragmentState {
                            module: &self.module,
                            entry_point: shader_entry_name,
                            compilation_options: Default::default(),
                            targets: &[Some(wgpu::ColorTargetState {
                                format: wgpu::TextureFormat::from(color_format),
                                blend: None,
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
                    }),
                )
            })
            .clone()
    }
}

pub struct Texture {
    pub desc: ImageDesc,
    pub(crate) texture: wgpu::Texture,
    pub(crate) view: wgpu::TextureView,
    pub(crate) extent: wgpu::Extent3d,
}

impl Texture {}

struct BufferImage {
    buffer: wgpu::Buffer,
    image_index: (usize, usize), // action index, index of (tex, img) inside action vec
}

pub struct TextureWithTransform {
    pub(crate) texture: Texture,
    pub(crate) transform: Transform2D,
}

impl TextureWithTransform {
    pub(crate) fn from_texture(texture: Texture) -> Self {
        Self {
            texture,
            transform: Transform2D::default(),
        }
    }
    pub(crate) fn new(texture: Texture, transform: Transform2D) -> Self {
        Self { texture, transform }
    }
}
