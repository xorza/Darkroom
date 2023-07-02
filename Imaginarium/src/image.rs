use std::fs::File;
use std::path::Path;

use bytemuck::Pod;
use image::EncodableLayout;
use num_traits::{Bounded, NumCast, ToPrimitive};
use tiff::decoder::DecodingResult;

use crate::image_convert::{*};

#[derive(Debug, PartialEq, Eq, Copy, Clone, Default)]
#[repr(u32)]
pub enum ChannelCount {
    Gray = 1,
    GrayAlpha = 2,
    #[default]
    Rgba = 3,
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

#[derive(Clone, Default)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub channel_count: ChannelCount,
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

fn align_up_2(n: u32) -> u32 {
    // (n + 3) & !3
    if n % 2 == 0 {
        n
    } else {
        n + 1
    }
}

impl Image {
    pub fn new(
        width: u32,
        height: u32,
        channel_count: ChannelCount,
        channel_size: ChannelSize,
        channel_type: ChannelType)
        -> Image
    {
        let stride = align_up_2(width * channel_count as u32 * channel_size as u32);
        let bytes = vec![0; (stride * height) as usize];

        Image {
            width,
            height,
            stride,
            channel_count,
            channel_size,
            channel_type,
            bytes,
        }
    }

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

        let (channel_count, channel_size, channel_type) = match img.color() {
            // @formatter:off
            image::ColorType::L8      => (ChannelCount::Gray,      ChannelSize::_8bit,  ChannelType::Int   ),
            image::ColorType::L16     => (ChannelCount::Gray,      ChannelSize::_16bit, ChannelType::Int   ),
            image::ColorType::La8     => (ChannelCount::GrayAlpha, ChannelSize::_8bit,  ChannelType::Int   ),
            image::ColorType::La16    => (ChannelCount::GrayAlpha, ChannelSize::_16bit, ChannelType::Int   ),
            image::ColorType::Rgb8    => (ChannelCount::Rgba,      ChannelSize::_8bit,  ChannelType::Int   ),
            image::ColorType::Rgba8   => (ChannelCount::Rgba,      ChannelSize::_8bit,  ChannelType::Int   ),
            image::ColorType::Rgb16   => (ChannelCount::Rgba,      ChannelSize::_16bit, ChannelType::Int   ),
            image::ColorType::Rgba16  => (ChannelCount::Rgba,      ChannelSize::_16bit, ChannelType::Int   ),
            image::ColorType::Rgb32F  => (ChannelCount::Rgba,      ChannelSize::_32bit, ChannelType::Float ),
            image::ColorType::Rgba32F => (ChannelCount::Rgba,      ChannelSize::_32bit, ChannelType::Float ),
            _ =>  panic!("Unsupported color type: {:?}", img.color()),
            // @formatter:on
        };

        let image = Image {
            width: img.width(),
            height: img.height(),
            stride: bytes.len() as u32 / img.height(),
            channel_count,
            channel_size,
            channel_type,
            bytes,
        };

        Ok(image)
    }

    fn load_tiff(filename: &str) -> anyhow::Result<Image> {
        let mut decoder = tiff::decoder::Decoder::new(
            File::open(filename)?
        )?;

        let (read_channel_count, channel_bits, supported_channel_count) = match decoder.colortype()? {
            // @formatter:off
            tiff::ColorType::Gray  (b) => (1, b, ChannelCount::Gray      ),
            tiff::ColorType::GrayA (b) => (2, b, ChannelCount::GrayAlpha ),
            tiff::ColorType::RGB   (b) => (3, b, ChannelCount::Rgba      ),
            tiff::ColorType::RGBA  (b) => (4, b, ChannelCount::Rgba      ),
            _ => panic!("Unsupported color type: {:?}", decoder.colortype()?),
            // @formatter:on
        };

        let img = decoder.read_image()?;
        let bytes: Vec<u8> = match &img {
            // @formatter:off
            DecodingResult::U8 (buf) => align_channels(buf, read_channel_count, u8::MAX),
            DecodingResult::I8 (buf) => align_channels(buf, read_channel_count, i8::max_value()),
            DecodingResult::U16(buf) => align_channels(buf, read_channel_count, u16::MAX),
            DecodingResult::I16(buf) => align_channels(buf, read_channel_count, i16::max_value()),
            DecodingResult::U32(buf) => align_channels(buf, read_channel_count, u32::MAX),
            DecodingResult::I32(buf) => align_channels(buf, read_channel_count, i32::max_value()),
            DecodingResult::U64(buf) => align_channels(buf, read_channel_count, u64::MAX),
            DecodingResult::I64(buf) => align_channels(buf, read_channel_count, i64::max_value()),
            DecodingResult::F32(buf) => align_channels(buf, read_channel_count, 1.0f32),
            DecodingResult::F64(buf) => align_channels(buf, read_channel_count, 1.0f64),
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
            channel_count: supported_channel_count,
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

    fn save_jpg(&self, filename: &str) -> anyhow::Result<()> {
        if self.channel_type != ChannelType::Int {
            return Err(anyhow::anyhow!("Unsupported JPEG channel type: {:?}", self.channel_type));
        }

        let color_type = match self.channel_size {
            ChannelSize::_8bit => match self.channel_count {
                ChannelCount::Gray => image::ColorType::L8,
                // ColorFormat::Rgba => image::ColorType::Rgb8,

                _ => return Err(anyhow::anyhow!("Unsupported JPEG color format: {:?}", self.channel_count)),
            },

            _ => return Err(anyhow::anyhow!("Unsupported JPEG channel size: {:?}", self.channel_size)),
        };

        image::save_buffer(filename, &self.bytes, self.width, self.height, color_type)?;

        Ok(())
    }

    fn save_png(&self, filename: &str) -> anyhow::Result<()> {
        if self.channel_type != ChannelType::Int {
            return Err(anyhow::anyhow!("Unsupported PNG channel type: {:?}", self.channel_type));
        }

        let color_type = match self.channel_size {
            ChannelSize::_8bit => match self.channel_count {
                ChannelCount::Gray => image::ColorType::L8,
                ChannelCount::GrayAlpha => image::ColorType::La8,
                ChannelCount::Rgba => image::ColorType::Rgba8,
            },
            ChannelSize::_16bit => match self.channel_count {
                ChannelCount::Gray => image::ColorType::L16,
                ChannelCount::GrayAlpha => image::ColorType::La16,
                ChannelCount::Rgba => image::ColorType::Rgba16,
            },

            _ => return Err(anyhow::anyhow!("Unsupported PNG channel size: {:?}", self.channel_size)),
        };

        image::save_buffer(filename, &self.bytes, self.width, self.height, color_type)?;

        Ok(())
    }


    pub fn convert(
        &mut self,
        channel_count: ChannelCount,
        channel_size: ChannelSize,
        channel_type: ChannelType)
        -> anyhow::Result<Image>
    {
        if channel_type == ChannelType::Float {
            match channel_size {
                ChannelSize::_8bit | ChannelSize::_16bit =>
                    return Err(anyhow::anyhow!("Unsupported channel size for float: {:?}", channel_size)),
                _ => {}
            }
        }

        let mut result = Image::new(
            self.width,
            self.height,
            channel_count,
            channel_size,
            channel_type,
        );

        match self.channel_size {
            // @formatter:off
            ChannelSize::_8bit =>
                match result.channel_size {
                    ChannelSize:: _8bit => convert::<u8,  u8>(self, &mut result,  u8_to_u8, avg_u8),
                    ChannelSize::_16bit => convert::<u8, u16>(self, &mut result, u8_to_u16, avg_u8),
                    ChannelSize::_32bit => convert::<u8, u32>(self, &mut result, u8_to_u32, avg_u8),
                    ChannelSize::_64bit => convert::<u8, u64>(self, &mut result, u8_to_u64, avg_u8),
                }
            ChannelSize::_16bit =>
                match result.channel_size {
                    ChannelSize:: _8bit => convert::<u16,  u8>(self, &mut result, u16_to_u8 , avg_u16),
                    ChannelSize::_16bit => convert::<u16, u16>(self, &mut result, u16_to_u16, avg_u16),
                    ChannelSize::_32bit => convert::<u16, u32>(self, &mut result, u16_to_u32, avg_u16),
                    ChannelSize::_64bit => convert::<u16, u64>(self, &mut result, u16_to_u64, avg_u16),
                }
            ChannelSize::_32bit =>
                match result.channel_size {
                    ChannelSize:: _8bit => convert::<u32,  u8>(self, &mut result, u32_to_u8 , avg_u32),
                    ChannelSize::_16bit => convert::<u32, u16>(self, &mut result, u32_to_u16, avg_u32),
                    ChannelSize::_32bit => convert::<u32, u32>(self, &mut result, u32_to_u32, avg_u32),
                    ChannelSize::_64bit => convert::<u32, u64>(self, &mut result, u32_to_u64, avg_u32),
                }
            ChannelSize::_64bit =>
                match result.channel_size {
                    ChannelSize:: _8bit => convert::<u64,  u8>(self, &mut result, u64_to_u8 , avg_u64),
                    ChannelSize::_16bit => convert::<u64, u16>(self, &mut result, u64_to_u16, avg_u64),
                    ChannelSize::_32bit => convert::<u64, u32>(self, &mut result, u64_to_u32, avg_u64),
                    ChannelSize::_64bit => convert::<u64, u64>(self, &mut result, u64_to_u64, avg_u64),
                }
            // @formatter:on
        }

        Ok(result)
    }
}

impl ChannelSize {
    pub(crate) fn byte_count(&self) -> u32 {
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

impl ChannelCount {
    fn channel_count(&self) -> u32 {
        match self {
            ChannelCount::Gray => 1,
            ChannelCount::GrayAlpha => 2,
            ChannelCount::Rgba => 4,
        }
    }
    pub(crate) fn byte_count(&self, channel_size: ChannelSize) -> u32 {
        self.channel_count() * channel_size.byte_count()
    }
}


