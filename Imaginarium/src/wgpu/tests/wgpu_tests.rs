use crate::image::Image;
use crate::wgpu::wgpu_context::{Shader, TextureSize, WgpuContext};

#[test]
fn it_works() {
    let img1 = Image::read_file("../test_resources/rainbow256x256.png").unwrap();
    let img2 = Image::read_file("../test_resources/squares256x256.png").unwrap();

    let context = WgpuContext::new().unwrap();
    let device = &context.device;
    let queue = &context.queue;


    let src_tex1_extent = wgpu::Extent3d {
        width: img1.width,
        height: img1.height,
        depth_or_array_layers: 1,
    };
    let src_tex1 = device.create_texture(&wgpu::TextureDescriptor {
        size: src_tex1_extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
        label: Some("tex 1"),
    });
    let src_tex1_view = src_tex1.create_view(&wgpu::TextureViewDescriptor::default());
    queue.write_texture(
        src_tex1.as_image_copy(),
        &img1.bytes,
        wgpu::ImageDataLayout {
            offset: 0,
            bytes_per_row: Some(img1.stride),
            rows_per_image: None,
        },
        src_tex1_extent,
    );

    let src_tex2_extent = wgpu::Extent3d {
        width: img2.width,
        height: img2.height,
        depth_or_array_layers: 1,
    };
    let src_tex2 = device.create_texture(&wgpu::TextureDescriptor {
        size: src_tex2_extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
        label: Some("tex 2"),
    });
    let src_tex2_view = src_tex2.create_view(&wgpu::TextureViewDescriptor::default());
    queue.write_texture(
        src_tex2.as_image_copy(),
        &img2.bytes,
        wgpu::ImageDataLayout {
            offset: 0,
            bytes_per_row: Some(img2.stride),
            rows_per_image: None,
        },
        src_tex2_extent,
    );


    let dst_tex_extent = wgpu::Extent3d {
        width: img2.width,
        height: img2.height,
        depth_or_array_layers: 1,
    };
    let dst_tex = device.create_texture(&wgpu::TextureDescriptor {
        size: dst_tex_extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
        label: Some("target texture"),
    });
    let dst_tex_view = dst_tex.create_view(&wgpu::TextureViewDescriptor::default());


    let shader = Shader::new(device, include_str!("shader.wgsl"));
    let texture_size = [
        TextureSize([img1.width as f32, img1.height as f32]),
        TextureSize([img2.width as f32, img2.height as f32]),
    ];


    let mut encoder = device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });

    context.draw_one(
        &mut encoder,
        &shader,
        &src_tex1_view,
        &src_tex2_view,
        &dst_tex_view,
        &texture_size,
    );


    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        size: img2.bytes.len() as wgpu::BufferAddress,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
        label: Some("Read buffer"),
    });
    encoder.copy_texture_to_buffer(
        wgpu::ImageCopyTexture {
            texture: &dst_tex,
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
        dst_tex_extent,
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
        img2.color_format,
        result,
    ).unwrap();
    img3.save_file("../test_output/compute.png").unwrap();
}
