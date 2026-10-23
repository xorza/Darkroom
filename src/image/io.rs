use std::io::{BufRead, Seek, Write};

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
use crate::image::file_format::FileFormat;
use crate::image::image_desc::ImageDesc;

/// A sink an encoder writes into and seeks back over, as the TIFF and PNG encoders do.
pub(super) trait WriteSeek: Write + Seek {}

impl<T: Write + Seek> WriteSeek for T {}

/// A source a decoder reads in buffered steps and seeks back over, as the TIFF decoder does.
pub(super) trait BufReadSeek: BufRead + Seek {}

impl<T: BufRead + Seek> BufReadSeek for T {}

pub(super) fn load(input: &mut dyn BufReadSeek, format: FileFormat) -> Result<Image> {
    match format {
        FileFormat::Png => load_png_jpeg(input, image_lib::ImageFormat::Png),
        FileFormat::Jpeg => load_png_jpeg(input, image_lib::ImageFormat::Jpeg),
        FileFormat::Tiff => load_tiff(input),
    }
}

pub(super) fn save(image: &Image, format: FileFormat, out: &mut dyn WriteSeek) -> Result<()> {
    match format {
        FileFormat::Png => save_png(image, out),
        FileFormat::Jpeg => save_jpg(image, out),
        FileFormat::Tiff => save_tiff(image, out),
    }
}

fn load_png_jpeg(input: &mut dyn BufReadSeek, format: image_lib::ImageFormat) -> Result<Image> {
    use image_lib::DynamicImage;

    let mut reader = image_lib::ImageReader::with_format(input, format);
    // A decode allocates what the header states, as a TIFF's does: the codec's default cap of
    // 512 MiB would refuse a large image the machine can hold.
    reader.no_limits();
    let img = reader.decode()?;
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

fn load_tiff(input: &mut dyn BufReadSeek) -> Result<Image> {
    // Use unlimited to support large astrophotography images
    let limits = Limits::unlimited();
    let mut decoder = Decoder::new(input)?.with_limits(limits);

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
    let fit = |extent: usize| {
        u32::try_from(extent).map_err(|_| {
            Error::UnsupportedFormat(format!("{desc} is larger than an image file can hold"))
        })
    };
    Ok([fit(desc.width)?, fit(desc.height)?])
}

fn save_jpg(image: &Image, mut out: &mut dyn WriteSeek) -> Result<()> {
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
    image_lib::write_buffer_with_format(
        &mut out,
        image.bytes(),
        width,
        height,
        color_type,
        image_lib::ImageFormat::Jpeg,
    )?;
    Ok(())
}

fn save_png(image: &Image, mut out: &mut dyn WriteSeek) -> Result<()> {
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
    image_lib::write_buffer_with_format(
        &mut out,
        image.bytes(),
        width,
        height,
        color_type,
        image_lib::ImageFormat::Png,
    )?;
    Ok(())
}

fn save_tiff(image: &Image, out: &mut dyn WriteSeek) -> Result<()> {
    let format = image.desc().color_format;
    match (format.channel_count, format.sample_type) {
        (ChannelCount::L, SampleType::U8) => write_tiff::<Gray8>(image, out),
        (ChannelCount::L, SampleType::U16) => write_tiff::<Gray16>(image, out),
        (ChannelCount::L, SampleType::F32) => write_tiff::<Gray32Float>(image, out),
        (ChannelCount::Rgb, SampleType::U8) => write_tiff::<RGB8>(image, out),
        (ChannelCount::Rgb, SampleType::U16) => write_tiff::<RGB16>(image, out),
        (ChannelCount::Rgb, SampleType::F32) => write_tiff::<RGB32Float>(image, out),
        (ChannelCount::Rgba, SampleType::U8) => write_tiff::<RGBA8>(image, out),
        (ChannelCount::Rgba, SampleType::U16) => write_tiff::<RGBA16>(image, out),
        (ChannelCount::Rgba, SampleType::F32) => write_tiff::<RGBA32Float>(image, out),
    }
}

/// Writes `image` as the TIFF color type `C`, whose sample type the caller matched to the
/// image's format — so the cast of the image's own aligned storage cannot fail.
fn write_tiff<C>(image: &Image, out: &mut dyn WriteSeek) -> Result<()>
where
    C: ColorType,
    C::Inner: Pod,
    [C::Inner]: TiffValue,
{
    let samples: &[C::Inner] = bytemuck::cast_slice(image.bytes());
    let [width, height] = encoder_dimensions(image)?;
    let mut tiff = TiffEncoder::new(out)?;
    tiff.new_image::<C>(width, height)?.write_data(samples)?;
    Ok(())
}
