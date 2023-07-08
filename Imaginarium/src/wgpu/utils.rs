use crate::color_format::ColorFormat;

impl From<wgpu::TextureFormat> for ColorFormat {
    fn from(value: wgpu::TextureFormat) -> Self {
        match value {
            wgpu::TextureFormat::R8Unorm => ColorFormat::GRAY_U8,
            wgpu::TextureFormat::R8Snorm => ColorFormat::GRAY_I8,

            wgpu::TextureFormat::Rgba8Unorm => ColorFormat::RGBA_U8,
            wgpu::TextureFormat::Rgba8Snorm => ColorFormat::RGBA_I8,

            _ => panic!("Not implemented texture format: {:?}", value),
        }
    }
}


impl From<&ColorFormat> for wgpu::TextureFormat {
    fn from(value: &ColorFormat) -> Self {
        match value {
            &ColorFormat::GRAY_U8 => wgpu::TextureFormat::R8Unorm,
            &ColorFormat::GRAY_I8 => wgpu::TextureFormat::R8Snorm,

            &ColorFormat::RGBA_U8 => wgpu::TextureFormat::Rgba8Unorm,
            &ColorFormat::RGBA_I8 => wgpu::TextureFormat::Rgba8Snorm,

            _ => panic!("Not implemented color format: {:?}", value.to_string()),
        }
    }
}


