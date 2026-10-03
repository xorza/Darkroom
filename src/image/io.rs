use std::fs::File;
use std::path::Path;

use bytemuck::Pod;
use image as image_lib;
use tiff::decoder::{Decoder, DecodingResult, Limits};
use tiff::encoder::colortype::{
    ColorType, Gray8, Gray16, Gray32Float, RGB8, RGB16, RGB32Float, RGBA8, RGBA16, RGBA32Float,
};
use tiff::encoder::{TiffEncoder, TiffValue};

use crate::common::color_format::{ChannelCount, ColorFormat, SampleType};
use crate::common::error::{Error, Result};
use crate::image::Image;
use crate::image::image_desc::ImageDesc;

pub(super) fn load_png_jpeg(filename: &Path) -> Result<Image> {
    use image_lib::DynamicImage;

    let img = image_lib::open(filename)?;
    let (width, height) = (img.width() as usize, img.height() as usize);
    let desc = |format| ImageDesc::new(width, height, format);
    // Each decoded buffer is handed over whole. Grey with alpha has no format of its own here;
    // it widens to RGBA, which keeps the alpha.
    match img {
        DynamicImage::ImageLuma8(buffer) => {
            Image::from_samples(desc(ColorFormat::L_U8), buffer.into_raw())
        }
        DynamicImage::ImageLuma16(buffer) => {
            Image::from_samples(desc(ColorFormat::L_U16), buffer.into_raw())
        }
        DynamicImage::ImageRgb8(buffer) => {
            Image::from_samples(desc(ColorFormat::RGB_U8), buffer.into_raw())
        }
        DynamicImage::ImageRgb16(buffer) => {
            Image::from_samples(desc(ColorFormat::RGB_U16), buffer.into_raw())
        }
        DynamicImage::ImageRgb32F(buffer) => {
            Image::from_samples(desc(ColorFormat::RGB_F32), buffer.into_raw())
        }
        DynamicImage::ImageRgba8(buffer) => {
            Image::from_samples(desc(ColorFormat::RGBA_U8), buffer.into_raw())
        }
        DynamicImage::ImageRgba16(buffer) => {
            Image::from_samples(desc(ColorFormat::RGBA_U16), buffer.into_raw())
        }
        DynamicImage::ImageRgba32F(buffer) => {
            Image::from_samples(desc(ColorFormat::RGBA_F32), buffer.into_raw())
        }
        DynamicImage::ImageLumaA8(_) => {
            Image::from_samples(desc(ColorFormat::RGBA_U8), img.into_rgba8().into_raw())
        }
        DynamicImage::ImageLumaA16(_) => {
            Image::from_samples(desc(ColorFormat::RGBA_U16), img.into_rgba16().into_raw())
        }
        img => Err(Error::UnsupportedColorType(format!("{:?}", img.color()))),
    }
}

pub(super) fn load_tiff(filename: &Path) -> Result<Image> {
    // Use unlimited to support large astrophotography images
    let limits = Limits::unlimited();
    let mut decoder = Decoder::new(File::open(filename)?)?.with_limits(limits);

    let channel_count = match decoder.colortype()? {
        tiff::ColorType::Gray(_) => ChannelCount::L,
        tiff::ColorType::RGB(_) => ChannelCount::Rgb,
        tiff::ColorType::RGBA(_) => ChannelCount::Rgba,
        color => return Err(Error::UnsupportedColorType(format!("{color:?}"))),
    };
    let (width, height) = decoder.dimensions()?;
    let desc = |sample_type| {
        ImageDesc::new(
            width as usize,
            height as usize,
            ColorFormat::new(channel_count, sample_type),
        )
    };

    // The decoded samples are handed over whole.
    match decoder.read_image()? {
        DecodingResult::U8(samples) => Image::from_samples(desc(SampleType::U8), samples),
        DecodingResult::U16(samples) => Image::from_samples(desc(SampleType::U16), samples),
        DecodingResult::F32(samples) => Image::from_samples(desc(SampleType::F32), samples),
        result => Err(Error::UnsupportedFormat(format!(
            "TIFF sample format not supported: {}",
            sample_format_name(&result)
        ))),
    }
}

/// The sample type a TIFF decoded to, without the samples.
const fn sample_format_name(result: &DecodingResult) -> &'static str {
    match result {
        DecodingResult::U8(_) => "u8",
        DecodingResult::U16(_) => "u16",
        DecodingResult::U32(_) => "u32",
        DecodingResult::U64(_) => "u64",
        DecodingResult::I8(_) => "i8",
        DecodingResult::I16(_) => "i16",
        DecodingResult::I32(_) => "i32",
        DecodingResult::I64(_) => "i64",
        DecodingResult::F16(_) => "f16",
        DecodingResult::F32(_) => "f32",
        DecodingResult::F64(_) => "f64",
    }
}

/// The dimensions as the `u32` the encoders take.
fn encoder_dimensions(image: &Image) -> Result<[u32; 2]> {
    let desc = image.desc();
    #[expect(
        clippy::map_err_ignore,
        reason = "`TryFromIntError` says only that the value did not fit, which the message says"
    )]
    let fit = |extent: usize| {
        u32::try_from(extent).map_err(|_| {
            Error::UnsupportedFormat(format!("{desc} is larger than an image file can hold"))
        })
    };
    Ok([fit(desc.width)?, fit(desc.height)?])
}

pub(super) fn save_jpg(image: &Image, filename: &Path) -> Result<()> {
    let format = image.desc().color_format;
    let color_type = match format {
        ColorFormat::L_U8 => image_lib::ColorType::L8,
        ColorFormat::RGB_U8 => image_lib::ColorType::Rgb8,
        _ => {
            return Err(Error::UnsupportedFormat(format!(
                "JPEG cannot store {format}"
            )));
        }
    };
    let [width, height] = encoder_dimensions(image)?;
    image_lib::save_buffer_with_format(
        filename,
        image.bytes(),
        width,
        height,
        color_type,
        image_lib::ImageFormat::Jpeg,
    )?;
    Ok(())
}

pub(super) fn save_png(image: &Image, filename: &Path) -> Result<()> {
    let format = image.desc().color_format;
    let color_type = match (format.channel_count, format.sample_type) {
        (ChannelCount::L, SampleType::U8) => image_lib::ColorType::L8,
        (ChannelCount::Rgb, SampleType::U8) => image_lib::ColorType::Rgb8,
        (ChannelCount::Rgba, SampleType::U8) => image_lib::ColorType::Rgba8,
        (ChannelCount::L, SampleType::U16) => image_lib::ColorType::L16,
        (ChannelCount::Rgb, SampleType::U16) => image_lib::ColorType::Rgb16,
        (ChannelCount::Rgba, SampleType::U16) => image_lib::ColorType::Rgba16,
        (_, SampleType::F32) => {
            return Err(Error::UnsupportedFormat(format!(
                "PNG cannot store {format}"
            )));
        }
    };
    let [width, height] = encoder_dimensions(image)?;
    image_lib::save_buffer_with_format(
        filename,
        image.bytes(),
        width,
        height,
        color_type,
        image_lib::ImageFormat::Png,
    )?;
    Ok(())
}

pub(super) fn save_tiff(image: &Image, filename: &Path) -> Result<()> {
    let format = image.desc().color_format;
    match (format.channel_count, format.sample_type) {
        (ChannelCount::L, SampleType::U8) => write_tiff::<Gray8>(image, filename),
        (ChannelCount::L, SampleType::U16) => write_tiff::<Gray16>(image, filename),
        (ChannelCount::L, SampleType::F32) => write_tiff::<Gray32Float>(image, filename),
        (ChannelCount::Rgb, SampleType::U8) => write_tiff::<RGB8>(image, filename),
        (ChannelCount::Rgb, SampleType::U16) => write_tiff::<RGB16>(image, filename),
        (ChannelCount::Rgb, SampleType::F32) => write_tiff::<RGB32Float>(image, filename),
        (ChannelCount::Rgba, SampleType::U8) => write_tiff::<RGBA8>(image, filename),
        (ChannelCount::Rgba, SampleType::U16) => write_tiff::<RGBA16>(image, filename),
        (ChannelCount::Rgba, SampleType::F32) => write_tiff::<RGBA32Float>(image, filename),
    }
}

/// Writes `image` as the TIFF colour type `C`, whose sample type the caller matched to the
/// image's format — so the cast of the image's own aligned storage cannot fail.
fn write_tiff<C>(image: &Image, filename: &Path) -> Result<()>
where
    C: ColorType,
    C::Inner: Pod,
    [C::Inner]: TiffValue,
{
    let samples: &[C::Inner] = bytemuck::cast_slice(image.bytes());
    let [width, height] = encoder_dimensions(image)?;
    let mut file = File::create(filename)?;
    let mut tiff = TiffEncoder::new(&mut file)?;
    tiff.new_image::<C>(width, height)?.write_data(samples)?;
    Ok(())
}
