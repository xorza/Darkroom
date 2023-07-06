use crate::image::Image;
use crate::wgpu::math::TextureTransform;
use crate::wgpu::wgpu_context::{Shader, Texture, WgpuContext};

#[test]
fn it_works() {
    let context = WgpuContext::new().unwrap();
    let device = &context.device;
    let queue = &context.queue;


    let img = Image::read_file("../test_resources/rainbow256x256.png").unwrap();
    let image_desc = img.desc.clone();
    let tex1 = Texture::new(device, &image_desc);
    tex1.write(queue, &img).unwrap();
    drop(img);

    let img = Image::read_file("../test_resources/squares256x256.png").unwrap();
    let tex2 = Texture::new(device, &image_desc);
    tex2.write(queue, &img).unwrap();
    drop(img);

    let dst_tex = Texture::new(device, &image_desc);

    let mut texture_transforms = [
        TextureTransform::default(),
        TextureTransform::default()
    ];
    texture_transforms[1]
        .aspect(1.0)
        .translate(0.5, 0.5)
        .rotate(-1.0)
        .translate(-0.5, -0.5);

    let shader = Shader::new(
        device,
        include_str!("shader.wgsl"),
        2,
        std::mem::size_of_val(&texture_transforms) as u32,
        &[wgpu::VertexFormat::Float32x2, wgpu::VertexFormat::Float32x2],
    );

    let mut encoder = device.create_command_encoder(
        &wgpu::CommandEncoderDescriptor::default()
    );
    context.draw_one(
        &mut encoder,
        &shader,
        &tex1.view,
        &tex2.view,
        &dst_tex.view,
        &texture_transforms,
    );
    queue.submit(Some(encoder.finish()));

    let mut img3 = Image::new_empty(image_desc).unwrap();
    dst_tex.read(device, queue, &mut img3).unwrap();
    img3.save_file("../test_output/compute.png").unwrap();
}
