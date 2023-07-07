use crate::image::Image;
use crate::wgpu::math::TextureTransform;
use crate::wgpu::wgpu_context::{Action, WgpuContext};

#[test]
fn it_works() {
    let context = WgpuContext::new().unwrap();
    let device = &context.device;
    let queue = &context.queue;


    let img = Image::read_file("../test_resources/rainbow256x256.png").unwrap();
    let image_desc = img.desc.clone();
    let input_tex1 = context.create_texture(image_desc.clone());
    input_tex1.write(queue, &img).unwrap();
    drop(img);

    let img = Image::read_file("../test_resources/squares256x256.png").unwrap();
    let input_tex2 = context.create_texture(image_desc.clone());
    input_tex2.write(queue, &img).unwrap();
    drop(img);

    let output_tex = context.create_texture(image_desc.clone());

    let mut texture_transforms = [
        TextureTransform::default(),
        TextureTransform::default()
    ];
    texture_transforms[1]
        .aspect(1, 1)
        .center()
        .rotate(-1.0)
        .uncenter();

    let shader = context.create_shader(
        include_str!("shader.wgsl"),
        2,
        std::mem::size_of::<[TextureTransform; 2]>() as u32,
    );

    let mut encoder = device.create_command_encoder(
        &wgpu::CommandEncoderDescriptor::default()
    );
    context.run_shader(
        &mut encoder,
        &shader,
        &[&input_tex1, &input_tex2],
        &output_tex,
        bytemuck::bytes_of(&texture_transforms),
    );
    queue.submit(Some(encoder.finish()));

    let mut img3 = Image::new_empty(image_desc).unwrap();
    output_tex.read(device, queue, &mut img3).unwrap();
    img3.save_file("../test_output/compute.png").unwrap();
}

#[test]
fn it_works2() {
    let context = WgpuContext::new().unwrap();

    let mut texture_transforms = [
        TextureTransform::default(),
        TextureTransform::default()
    ];
    texture_transforms[1]
        .aspect(1, 1)
        .center()
        .rotate(-1.0)
        .uncenter();

    let img1 = Image::read_file("../test_resources/rainbow256x256.png").unwrap();
    let mut img2 = Image::read_file("../test_resources/squares256x256.png").unwrap();
    let mut img3 = Image::new_empty(img1.desc.clone()).unwrap();

    let tex1 = context.create_texture(img1.desc.clone());
    let tex2 = context.create_texture(img2.desc.clone());
    let tex3 = context.create_texture(img1.desc.clone());

    let shader = context.create_shader(
        include_str!("shader.wgsl"),
        2,
        std::mem::size_of_val(&texture_transforms) as u32,
    );

    context.perform(&[
        Action::ImgToTex {
            images: vec![&img1, &img2],
            textures: vec![&tex1, &tex2],
        }
    ]);

    context.perform(&[
        Action::RunShader {
            shader: &shader,
            input_textures: vec![&tex1, &tex2],
            output_texture: &tex3,
            push_constants: bytemuck::bytes_of(&texture_transforms),
        },
        Action::RunShader {
            shader: &shader,
            input_textures: vec![&tex3, &tex2],
            output_texture: &tex1,
            push_constants: bytemuck::bytes_of(&texture_transforms),
        },
        Action::tex_to_img(&[&tex3, &tex1], &mut [&mut img2, &mut img3])
    ]);

    img2.save_file("../test_output/compute2.png").unwrap();
    img3.save_file("../test_output/compute3.png").unwrap();
}