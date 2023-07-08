use std::cell::RefCell;

use crate::image::Image;
use crate::wgpu::math::Transform2D;
use crate::wgpu::wgpu_context::{Action, TextureWithTransform, WgpuContext};

#[test]
fn it_works2() {
    let context = WgpuContext::new().unwrap();

    let img1 = Image::read_file("../test_resources/rainbow256x256.png").unwrap();
    let mut img2 = Image::read_file("../test_resources/squares256x256.png").unwrap();
    let mut img3 = Image::new_empty(img1.desc.clone()).unwrap();

    let mut transform = Transform2D::default();
    transform
        .aspect(1, 1)
        .center()
        .rotate(-1.0)
        .uncenter();

    let tex1 = TextureWithTransform::from_texture(
        context.create_texture(img1.desc.clone()),
    );
    let tex2 = TextureWithTransform::new(
        context.create_texture(img2.desc.clone()),
        transform,
    );
    let tex3 = TextureWithTransform::from_texture(
        context.create_texture(img1.desc.clone()),
    );


    let shader = context.create_shader(
        include_str!("blend_frag.wgsl"),
        2,
        0,
    );

    context.perform(&[
        Action::ImgToTex(vec![(&img1, &tex1.texture), (&img2, &tex2.texture)])
    ]);

    context.perform(&[
        Action::RunShader {
            shader: &shader,
            shader_entry_name: "fs_main",
            input_textures: vec![&tex1, &tex2],
            output_texture: &tex3.texture,
            push_constants: &[],
        },
        Action::RunShader {
            shader: &shader,
            shader_entry_name: "fs_main",
            input_textures: vec![&tex3, &tex2],
            output_texture: &tex1.texture,
            push_constants: &[],
        },
        Action::TexToImg(vec![
            (&tex1.texture, RefCell::new(&mut img2)),
            (&tex3.texture, RefCell::new(&mut img3)),
        ]),
    ]);
    context.sync();

    img2.save_file("../test_output/compute1.png").unwrap();
    img3.save_file("../test_output/compute2.png").unwrap();
}