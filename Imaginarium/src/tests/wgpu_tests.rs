use std::borrow::Cow;

use wgpu::util::DeviceExt;

use crate::image::{ChannelCount, ChannelSize, ChannelType, Image};
use crate::wgpu_context::{Rect, Vert, WgpuContext};

#[test]
fn it_works() {
    let img1 = Image::read_file("../test_resources/rainbow256x256.png").unwrap();
    let img2 = Image::read_file("../test_resources/squares256x256.png").unwrap();

    let context = WgpuContext::new().unwrap();
    let device = &context.device;
    let queue = &context.queue;

    let rect = Rect::new(1.0, 1.0, img1.width as f32, img1.height as f32);

    let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        contents: rect.as_bytes(),
        usage: wgpu::BufferUsages::VERTEX,
        label: Some("Vertex Buffer"),
    });

    let tex1_extent = wgpu::Extent3d {
        width: img1.width,
        height: img1.height,
        depth_or_array_layers: 1,
    };
    let tex1 = device.create_texture(&wgpu::TextureDescriptor {
        size: tex1_extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
        label: Some("tex 1"),
    });
    let tex1_view = tex1.create_view(&wgpu::TextureViewDescriptor::default());
    queue.write_texture(
        tex1.as_image_copy(),
        &img1.bytes,
        wgpu::ImageDataLayout {
            offset: 0,
            bytes_per_row: Some(img1.stride),
            rows_per_image: None,
        },
        tex1_extent,
    );

    let tex2_extent = wgpu::Extent3d {
        width: img2.width,
        height: img2.height,
        depth_or_array_layers: 1,
    };
    let tex2 = device.create_texture(&wgpu::TextureDescriptor {
        size: tex2_extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
        label: Some("tex 2"),
    });
    let tex2_view = tex2.create_view(&wgpu::TextureViewDescriptor::default());
    queue.write_texture(
        tex2.as_image_copy(),
        &img2.bytes,
        wgpu::ImageDataLayout {
            offset: 0,
            bytes_per_row: Some(img2.stride),
            rows_per_image: None,
        },
        tex2_extent,
    );

    let bind_group_layout = device.create_bind_group_layout(
        &wgpu::BindGroupLayoutDescriptor {
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
    let pipeline_layout = device.create_pipeline_layout(
        &wgpu::PipelineLayoutDescriptor {
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
            label: None,
        });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        layout: &bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&tex1_view),
            },
        ],
        label: None,
    });

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(include_str!("shader.wgsl"))),
        label: None,
    });

    let vertex_buffers =
        [
            wgpu::VertexBufferLayout {
                array_stride: rect.stride() as wgpu::BufferAddress,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &[
                    wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x2,
                        offset: 0,
                        shader_location: 0,
                    },
                    wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x2,
                        offset: 4 * 2,
                        shader_location: 1,
                    },
                ],
            }
        ];

    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: "vs_main",
            buffers: &vertex_buffers,
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


    let mut encoder = device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });

    {
        let mut render_pass = encoder.begin_render_pass(
            &wgpu::RenderPassDescriptor {
                color_attachments: &[
                    Some(wgpu::RenderPassColorAttachment {
                        view: &tex2_view,
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
        render_pass.set_vertex_buffer(0, vertex_buffer.slice(..));
        render_pass.pop_debug_group();

        render_pass.set_bind_group(0, &bind_group, &[]);
        render_pass.insert_debug_marker("Draw.");
        render_pass.draw(0..rect.vert_count(), 0..1);
    }

    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        size: img2.bytes.len() as wgpu::BufferAddress,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
        label: Some("Read buffer"),
    });
    encoder.copy_texture_to_buffer(
        wgpu::ImageCopyTexture {
            texture: &tex2,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: Default::default(),
        },
        wgpu::ImageCopyBuffer {
            buffer: &buffer,
            layout: wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(img2.width * img2.bytes_per_pixel()),
                rows_per_image: Some(img2.height),
            },
        },
        tex2_extent,
    );

    queue.submit(Some(encoder.finish()));

    let slice = buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |result| { result.unwrap(); });
    device.poll(wgpu::Maintain::Wait);

    let data = slice.get_mapped_range();
    let result: Vec<u8> = data.to_vec();
    drop(data);

    buffer.unmap();


    let img3 = Image::new_with_data(
        img2.width,
        img2.height,
        img2.channel_count,
        img2.channel_size,
        img2.channel_type,
        result,
    );
    img3.save_file("../test_output/compute.png").unwrap();
}
