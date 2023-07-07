use crate::image::{Image, ImageDesc};
use crate::wgpu::math::TextureTransform;
use crate::wgpu::wgpu_context::Texture;

struct ImageTexture {
    desc: ImageDesc,
    img: Option<Image>,
    tex: Option<Texture>,
    transform: TextureTransform,
}