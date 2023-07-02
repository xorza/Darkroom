use std::fs::File;
use std::path::Path;

use bytemuck::Pod;
use image::EncodableLayout;
use tiff::decoder::DecodingResult;

#[derive(Debug, PartialEq, Eq, Copy, Clone, Default)]
pub enum ColorFormat {
    Gray,
    GrayAlpha,
    #[default]
    Rgba,
}

#[derive(Debug, PartialEq, Eq, Copy, Clone, Default)]
#[repr(u32)]
pub enum ChannelSize {
    #[default]
    _8bit = 1,
    _16bit = 2,
    _32bit = 4,
    _64bit = 8,
}

#[derive(Debug, PartialEq, Eq, Copy, Clone, Default)]
pub enum ChannelType {
    #[default]
    Int,
    Float,
}

#[derive(Default)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub color_format: ColorFormat,
    pub channel_size: ChannelSize,
    pub channel_type: ChannelType,
    pub bytes: Vec<u8>,
}


fn align_channels<T>(buf: &Vec<T>, channel_count: u32, filler: T) -> Vec<u8>
    where T: Copy + Pod
{
    let mut result: Vec<u8>;

    if channel_count == 3 {
        result = Vec::new();
        result.resize(std::mem::size_of::<T>() * buf.len() * 4 / 3, 0);

        let slice: &mut [T] = bytemuck::cast_slice_mut(&mut result);
        for i in 0..(buf.len() / 3) {
            slice[i * 4 + 0] = buf[i * 3 + 0];
            slice[i * 4 + 1] = buf[i * 3 + 1];
            slice[i * 4 + 2] = buf[i * 3 + 2];
            slice[i * 4 + 3] = filler;
        }
    } else {
        result = bytemuck::cast_slice(buf).to_vec()
    }

    result
}

fn get_file_extension(filename: &str) -> anyhow::Result<&str> {
    let extension = Path::new(filename)
        .extension()
        .and_then(|os_str| os_str.to_str())
        .ok_or(anyhow::anyhow!("Failed to get file extension"))?;

    Ok(extension)
}

impl Image {
    pub fn read_file(filename: &str) -> anyhow::Result<Image> {
        let extension = get_file_extension(filename)?;

        let image =
            match extension {
                "png" | "jpeg" | "jpg" =>
                    Image::load_png_jpeg(filename)?,
                "tiff" =>
                    Image::load_tiff(filename)?,
                _ => return Err(anyhow::anyhow!("Unsupported file extension: {}", extension)),
            };

        Ok(image)
    }

    fn load_png_jpeg(filename: &str) -> anyhow::Result<Image> {
        let img = image::open(filename)
            .expect("Failed to open image");

        let bytes = match img.color() {
            // @formatter:off
            image::ColorType::L8      => img.to_luma8()        .as_bytes().to_vec(),
            image::ColorType::L16     => img.to_luma16()       .as_bytes().to_vec(),
            image::ColorType::La8     => img.to_luma_alpha8()  .as_bytes().to_vec(),
            image::ColorType::La16    => img.to_luma_alpha16() .as_bytes().to_vec(),
            image::ColorType::Rgb8    => img.to_rgba8()        .as_bytes().to_vec(),
            image::ColorType::Rgba8   => img.to_rgba8()        .as_bytes().to_vec(),
            image::ColorType::Rgb16   => img.to_rgba16()       .as_bytes().to_vec(),
            image::ColorType::Rgba16  => img.to_rgba16()       .as_bytes().to_vec(),
            image::ColorType::Rgb32F  => img.to_rgba32f()      .as_bytes().to_vec(),
            image::ColorType::Rgba32F => img.to_rgba32f()      .as_bytes().to_vec(),
            _ =>  panic!("Unsupported color type: {:?}", img.color()),
            // @formatter:on
        };

        let (color_format, pixel_size, channel_type) = match img.color() {
            // @formatter:off
            image::ColorType::L8      => (ColorFormat::Gray,      ChannelSize::_8bit,  ChannelType::Int   ),
            image::ColorType::L16     => (ColorFormat::Gray,      ChannelSize::_16bit, ChannelType::Int   ),
            image::ColorType::La8     => (ColorFormat::GrayAlpha, ChannelSize::_8bit,  ChannelType::Int   ),
            image::ColorType::La16    => (ColorFormat::GrayAlpha, ChannelSize::_16bit, ChannelType::Int   ),
            image::ColorType::Rgb8    => (ColorFormat::Rgba,      ChannelSize::_8bit,  ChannelType::Int   ),
            image::ColorType::Rgba8   => (ColorFormat::Rgba,      ChannelSize::_8bit,  ChannelType::Int   ),
            image::ColorType::Rgb16   => (ColorFormat::Rgba,      ChannelSize::_16bit, ChannelType::Int   ),
            image::ColorType::Rgba16  => (ColorFormat::Rgba,      ChannelSize::_16bit, ChannelType::Int   ),
            image::ColorType::Rgb32F  => (ColorFormat::Rgba,      ChannelSize::_32bit, ChannelType::Float ),
            image::ColorType::Rgba32F => (ColorFormat::Rgba,      ChannelSize::_32bit, ChannelType::Float ),
            _ =>  panic!("Unsupported color type: {:?}", img.color()),
            // @formatter:on
        };

        let image = Image {
            width: img.width(),
            height: img.height(),
            stride: bytes.len() as u32 / img.height(),
            color_format,
            channel_size: pixel_size,
            channel_type,
            bytes,
        };

        Ok(image)
    }

    fn load_tiff(filename: &str) -> anyhow::Result<Image> {
        let mut decoder = tiff::decoder::Decoder::new(
            File::open(filename)?
        )?;

        let (channel_count, channel_bits, color_format) = match decoder.colortype()? {
            // @formatter:off
            tiff::ColorType::Gray  (b) => (1, b, ColorFormat::Gray      ),
            tiff::ColorType::GrayA (b) => (2, b, ColorFormat::GrayAlpha ),
            tiff::ColorType::RGB   (b) => (3, b, ColorFormat::Rgba      ),
            tiff::ColorType::RGBA  (b) => (4, b, ColorFormat::Rgba      ),
            _ => panic!("Unsupported color type: {:?}", decoder.colortype()?),
            // @formatter:on
        };

        let img = decoder.read_image()?;
        let bytes: Vec<u8> = match &img {
            // @formatter:off
            DecodingResult::U8 (buf) => align_channels(buf, channel_count, u8::MAX),
            DecodingResult::I8 (buf) => align_channels(buf, channel_count, i8::MAX),
            DecodingResult::U16(buf) => align_channels(buf, channel_count, u16::MAX),
            DecodingResult::I16(buf) => align_channels(buf, channel_count, i16::MAX),
            DecodingResult::U32(buf) => align_channels(buf, channel_count, u32::MAX),
            DecodingResult::I32(buf) => align_channels(buf, channel_count, i32::MAX),
            DecodingResult::U64(buf) => align_channels(buf, channel_count, u64::MAX),
            DecodingResult::I64(buf) => align_channels(buf, channel_count, i64::MAX),
            DecodingResult::F32(buf) => align_channels(buf, channel_count, 1.0f32),
            DecodingResult::F64(buf) => align_channels(buf, channel_count, 1.0f64),
            // @formatter:on
        };

        let channel_type = match &img {
            // @formatter:off
            DecodingResult::U8 (_) => ChannelType::Int,
            DecodingResult::I8 (_) => ChannelType::Int,
            DecodingResult::U16(_) => ChannelType::Int,
            DecodingResult::I16(_) => ChannelType::Int,
            DecodingResult::U32(_) => ChannelType::Int,
            DecodingResult::I32(_) => ChannelType::Int,
            DecodingResult::U64(_) => ChannelType::Int,
            DecodingResult::I64(_) => ChannelType::Int,
            DecodingResult::F32(_) => ChannelType::Float,
            DecodingResult::F64(_) => ChannelType::Float,
            // @formatter:on
        };

        let (w, h) = decoder.dimensions()?;

        let image = Image {
            width: w,
            height: h,
            stride: bytes.len() as u32 / h,
            color_format,
            channel_size: ChannelSize::from_bit_count(channel_bits as u32),
            channel_type,
            bytes,
        };

        Ok(image)
    }


    pub fn save_file(&self, filename: &str) -> anyhow::Result<()> {
        let extension = get_file_extension(filename)?;

        match extension {
            "png" => {
                self.save_png(filename)?;
            },
            // "jpeg" | "jpg" => {
            //     image::save_buffer(filename, &self.bytes, self.width, self.height, image::ColorType::Rgb8)?;
            // },
            // "tiff" => {},
            _ => return Err(anyhow::anyhow!("Unsupported file extension: {}", extension)),
        };

        Ok(())
    }

    fn save_png(&self, filename: &str) -> anyhow::Result<()> {
        if self.channel_type != ChannelType::Int {
            return Err(anyhow::anyhow!("Unsupported PNG channel type: {:?}", self.channel_type));
        }
        if self.channel_size != ChannelSize::_8bit {
            return Err(anyhow::anyhow!("Unsupported PNG channel size: {:?}", self.channel_size));
        }

        let color_type = match self.color_format {
            ColorFormat::Gray => image::ColorType::L8,
            ColorFormat::GrayAlpha => image::ColorType::La8,
            ColorFormat::Rgba => image::ColorType::Rgba8,
        };

        image::save_buffer(filename, &self.bytes, self.width, self.height, color_type)?;
        Ok(())
    }
}

impl ChannelSize {
    fn byte_count(&self) -> u32 {
        *self as u32
    }
    fn from_bit_count(bit_count: u32) -> ChannelSize {
        match bit_count {
            // @formatter:off
            8  => ChannelSize::_8bit ,
            16 => ChannelSize::_16bit,
            32 => ChannelSize::_32bit,
            62 => ChannelSize::_64bit,
            _  => panic!("Invalid channel size: {:?}", bit_count),
            // @formatter:on
        }
    }
}

impl ColorFormat {
    fn channel_count(&self) -> u32 {
        match self {
            ColorFormat::Gray => 1,
            ColorFormat::GrayAlpha => 2,
            ColorFormat::Rgba => 4,
        }
    }
    fn byte_count(&self, channel_size: ChannelSize) -> u32 {
        self.channel_count() * channel_size.byte_count()
    }
}


