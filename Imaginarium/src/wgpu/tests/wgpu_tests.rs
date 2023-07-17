use std::cell::RefCell;

use crate::image::Image;
use crate::math::Transform2D;
use crate::wgpu::wgpu_context::{Action, TextureWithTransform, WgpuContext};

#[test]
fn it_works2() {
    let context = WgpuContext::new().unwrap();

    let img1 = Image::read_file("../test_resources/rainbow256x256.png").unwrap();
    let img2 = Image::read_file("../test_resources/squares256x256.png").unwrap();

    let img_desc = img1.desc.clone();

    let mut transform = Transform2D::default();
    transform
        .aspect(1, 1)
        .center()
        .rotate(-1.0)
        .uncenter();

    let tex1 = TextureWithTransform::from_texture(
        context.create_texture(img_desc.clone()),
    );
    let tex2 = TextureWithTransform::new(
        context.create_texture(img_desc.clone()),
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

    drop(img1);
    drop(img2);

    context.perform(&[
        Action::RunShader {
            shader: &shader,
            shader_entry_name: "fs_mult_blend",
            input_textures: vec![&tex1, &tex2],
            output_texture: &tex3.texture,
            fragment_push_constant: &[],
        },
        Action::RunShader {
            shader: &shader,
            shader_entry_name: "fs_mult_blend",
            input_textures: vec![&tex3, &tex2],
            output_texture: &tex1.texture,
            fragment_push_constant: &[],
        },
    ]);

    drop(tex2);

    let mut img3 = Image::new_empty(img_desc.clone()).unwrap();
    let mut img4 = Image::new_empty(img_desc.clone()).unwrap();

    context.perform(&[
        Action::TexToImg(vec![
            (&tex1.texture, RefCell::new(&mut img3)),
            (&tex3.texture, RefCell::new(&mut img4)),
        ]),
    ]);

    context.sync();

    drop(tex1);
    drop(tex3);

    img3.save_file("../test_output/compute1.png").unwrap();
    img4.save_file("../test_output/compute2.png").unwrap();
}