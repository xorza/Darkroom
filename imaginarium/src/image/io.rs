use std::fs::File;
use std::path::Path;

use image as image_lib;
use tiff::decoder::DecodingResult;

use super::vec_to_avec;
use crate::prelude::*;

pub(crate) fn load_png_jpeg<P: AsRef<Path>>(filename: P) -> Result<Image> {
    let img = image_lib::open(filename)?;

    let (channel_count, channel_size, channel_type) = match img.color() {
        image_lib::ColorType::L8 => (ChannelCount::L, ChannelSize::_8bit, ChannelType::UInt),
        image_lib::ColorType::L16 => (ChannelCount::L, ChannelSize::_16bit, ChannelType::UInt),
        image_lib::ColorType::La8 => (ChannelCount::LA, ChannelSize::_8bit, ChannelType::UInt),
        image_lib::ColorType::La16 => (ChannelCount::LA, ChannelSize::_16bit, ChannelType::UInt),
        image_lib::ColorType::Rgb8 => (ChannelCount::Rgb, ChannelSize::_8bit, ChannelType::UInt),
        image_lib::ColorType::Rgb16 => (ChannelCount::Rgb, ChannelSize::_16bit, ChannelType::UInt),
        image_lib::ColorType::Rgba8 => (ChannelCount::Rgba, ChannelSize::_8bit, ChannelType::UInt),
        image_lib::ColorType::Rgba16 => {
            (ChannelCount::Rgba, ChannelSize::_16bit, ChannelType::UInt)
        }
        image_lib::ColorType::Rgb32F => {
            (ChannelCount::Rgb, ChannelSize::_32bit, ChannelType::Float)
        }
        image_lib::ColorType::Rgba32F => {
            (ChannelCount::Rgba, ChannelSize::_32bit, ChannelType::Float)
        }
        _ => return Err(Error::UnsupportedColorType(format!("{:?}", img.color()))),
    };

    let color_format = ColorFormat::from((channel_count, channel_size, channel_type));
    let desc = ImageDesc::new_packed(img.width() as usize, img.height() as usize, color_format);
    let bytes = vec_to_avec(img.into_bytes());

    Ok(Image { desc, bytes })
}

pub(crate) fn load_tiff<P: AsRef<Path>>(filename: P) -> Result<Image> {
    // Use unlimited to support large astrophotography images
    let limits = tiff::decoder::Limits::unlimited();
    let mut decoder = tiff::decoder::Decoder::new(File::open(filename)?)?.with_limits(limits);

    let (channel_bits, channel_count) = match decoder.colortype()? {
        tiff::ColorType::Gray(b) => (b, ChannelCount::L),
        tiff::ColorType::GrayA(b) => (b, ChannelCount::LA),
        tiff::ColorType::RGB(b) => (b, ChannelCount::Rgb),
        tiff::ColorType::RGBA(b) => (b, ChannelCount::Rgba),
        _ => {
            return Err(Error::UnsupportedColorType(format!(
                "{:?}",
                decoder.colortype()?
            )));
        }
    };

    let img = decoder.read_image()?;
    let bytes: Vec<u8> = match &img {
        DecodingResult::U8(buf) => bytemuck::cast_slice(buf).to_vec(),
        DecodingResult::U16(buf) => bytemuck::cast_slice(buf).to_vec(),
        DecodingResult::U32(buf) => bytemuck::cast_slice(buf).to_vec(),
        DecodingResult::F32(buf) => bytemuck::cast_slice(buf).to_vec(),
        DecodingResult::I8(_)
        | DecodingResult::I16(_)
        | DecodingResult::I32(_)
        | DecodingResult::I64(_)
        | DecodingResult::U64(_)
        | DecodingResult::F16(_)
        | DecodingResult::F64(_) => {
            return Err(Error::UnsupportedFormat(format!(
                "TIFF sample format not supported: {:?}",
                img
            )));
        }
    };

    let channel_type = match &img {
        DecodingResult::U8(_) | DecodingResult::U16(_) | DecodingResult::U32(_) => {
            ChannelType::UInt
        }
        DecodingResult::F32(_) => ChannelType::Float,
        _ => unreachable!(),
    };

    let (w, h) = decoder.dimensions()?;

    let channel_size = ChannelSize::from_bit_count(channel_bits)?;
    let color_format = ColorFormat::from((channel_count, channel_size, channel_type));
    let desc = ImageDesc::new_packed(w as usize, h as usize, color_format);
    let bytes = vec_to_avec(bytes);

    Ok(Image { desc, bytes })
}

pub(crate) fn save_jpg<P: AsRef<Path>>(image: &Image, filename: P) -> Result<()> {
    debug_assert!(
        image.desc().is_packed(),
        "Image must be packed before saving"
    );

    if image.desc.color_format.channel_type != ChannelType::UInt {
        return Err(Error::UnsupportedFormat(format!(
            "JPEG channel type: {:?}",
            image.desc.color_format.channel_type
        )));
    }

    let color_format = match image.desc.color_format.channel_size {
        ChannelSize::_8bit => match image.desc.color_format.channel_count {
            ChannelCount::L => image_lib::ColorType::L8,
            ChannelCount::Rgb => image_lib::ColorType::Rgb8,

            _ => {
                return Err(Error::UnsupportedFormat(format!(
                    "JPEG color format: {:?}",
                    image.desc.color_format.channel_count
                )));
            }
        },

        _ => {
            return Err(Error::UnsupportedFormat(format!(
                "JPEG channel size: {:?}",
                image.desc.color_format.channel_size
            )));
        }
    };

    image_lib::save_buffer_with_format(
        filename,
        image.bytes(),
        image.desc.width as u32,
        image.desc.height as u32,
        color_format,
        image_lib::ImageFormat::Jpeg,
    )?;

    Ok(())
}

pub(crate) fn save_png<P: AsRef<Path>>(image: &Image, filename: P) -> Result<()> {
    debug_assert!(
        image.desc().is_packed(),
        "Image must be packed before saving"
    );

    if image.desc.color_format.channel_type != ChannelType::UInt {
        return Err(Error::UnsupportedFormat(format!(
            "PNG channel type: {:?}",
            image.desc.color_format.channel_type
        )));
    }

    let color_format = match image.desc.color_format.channel_size {
        ChannelSize::_8bit => match image.desc.color_format.channel_count {
            ChannelCount::L => image_lib::ColorType::L8,
            ChannelCount::LA => image_lib::ColorType::La8,
            ChannelCount::Rgb => image_lib::ColorType::Rgb8,
            ChannelCount::Rgba => image_lib::ColorType::Rgba8,
        },
        ChannelSize::_16bit => match image.desc.color_format.channel_count {
            ChannelCount::L => image_lib::ColorType::L16,
            ChannelCount::LA => image_lib::ColorType::La16,
            ChannelCount::Rgb => image_lib::ColorType::Rgb16,
            ChannelCount::Rgba => image_lib::ColorType::Rgba16,
        },

        _ => {
            return Err(Error::UnsupportedFormat(format!(
                "PNG channel size: {:?}",
                image.desc.color_format.channel_size
            )));
        }
    };

    image_lib::save_buffer_with_format(
        filename,
        image.bytes(),
        image.desc.width as u32,
        image.desc.height as u32,
        color_format,
        image_lib::ImageFormat::Png,
    )?;

    Ok(())
}

pub(crate) use super::tiff::save_tiff;
