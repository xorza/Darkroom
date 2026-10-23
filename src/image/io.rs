use std::io::{BufRead, Seek, Write};

use bytemuck::Pod;
use image as image_lib;
use tiff::decoder::{Decoder, DecodingResult, DecodingSampleType, Limits};
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

/// The image `input` holds in `format`, once `admit` accepts the description its header gives:
/// `None` when it refuses, before any pixel is allocated or read.
pub(super) fn load(
    input: &mut dyn BufReadSeek,
    format: FileFormat,
    admit: &mut dyn FnMut(&ImageDesc) -> bool,
) -> Result<Option<Image>> {
    match format {
        FileFormat::Png => load_png_jpeg(input, image_lib::ImageFormat::Png, admit),
        FileFormat::Jpeg => load_png_jpeg(input, image_lib::ImageFormat::Jpeg, admit),
        FileFormat::Tiff => load_tiff(input, admit),
    }
}

pub(super) fn save(image: &Image, format: FileFormat, out: &mut dyn WriteSeek) -> Result<()> {
    match format {
        FileFormat::Png => save_png(image, out),
        FileFormat::Jpeg => save_jpg(image, out),
        FileFormat::Tiff => save_tiff(image, out),
    }
}

fn load_png_jpeg(
    input: &mut dyn BufReadSeek,
    format: image_lib::ImageFormat,
    admit: &mut dyn FnMut(&ImageDesc) -> bool,
) -> Result<Option<Image>> {
    use image_lib::{ColorType, DynamicImage, ImageDecoder};

    let mut reader = image_lib::ImageReader::with_format(input, format);
    // The admission bounds the decode, not the codec's default of 512 MiB.
    reader.no_limits();
    let decoder = reader.into_decoder()?;
    let (width, height) = decoder.dimensions();
    // Grey with alpha has no format of its own here; it widens to RGBA, which keeps the alpha.
    let color_format = match decoder.color_type() {
        ColorType::L8 => ColorFormat::L_U8,
        ColorType::L16 => ColorFormat::L_U16,
        ColorType::Rgb8 => ColorFormat::RGB_U8,
        ColorType::Rgb16 => ColorFormat::RGB_U16,
        ColorType::Rgb32F => ColorFormat::RGB_F32,
        ColorType::Rgba8 | ColorType::La8 => ColorFormat::RGBA_U8,
        ColorType::Rgba16 | ColorType::La16 => ColorFormat::RGBA_U16,
        ColorType::Rgba32F => ColorFormat::RGBA_F32,
        color => return Err(Error::UnsupportedColorType(format!("{color:?}"))),
    };
    let desc = ImageDesc::new(width as usize, height as usize, color_format);
    desc.validate()?;
    if !admit(&desc) {
        return Ok(None);
    }
    // Each decoded buffer is handed over whole.
    let image = match DynamicImage::from_decoder(decoder)? {
        DynamicImage::ImageLuma8(buffer) => Image::from_samples(desc, buffer.into_raw()),
        DynamicImage::ImageLuma16(buffer) => Image::from_samples(desc, buffer.into_raw()),
        DynamicImage::ImageRgb8(buffer) => Image::from_samples(desc, buffer.into_raw()),
        DynamicImage::ImageRgb16(buffer) => Image::from_samples(desc, buffer.into_raw()),
        DynamicImage::ImageRgb32F(buffer) => Image::from_samples(desc, buffer.into_raw()),
        DynamicImage::ImageRgba8(buffer) => Image::from_samples(desc, buffer.into_raw()),
        DynamicImage::ImageRgba16(buffer) => Image::from_samples(desc, buffer.into_raw()),
        DynamicImage::ImageRgba32F(buffer) => Image::from_samples(desc, buffer.into_raw()),
        img @ DynamicImage::ImageLumaA8(_) => {
            Image::from_samples(desc, img.into_rgba8().into_raw())
        }
        img @ DynamicImage::ImageLumaA16(_) => {
            Image::from_samples(desc, img.into_rgba16().into_raw())
        }
        _ => unreachable!("a decoder makes the color type its header names"),
    }?;
    Ok(Some(image))
}

fn load_tiff(
    input: &mut dyn BufReadSeek,
    admit: &mut dyn FnMut(&ImageDesc) -> bool,
) -> Result<Option<Image>> {
    // The header reads under the codec's default limits, which bound its tag arrays.
    let mut decoder = Decoder::new(input)?;
    let channel_count = match decoder.colortype()? {
        tiff::ColorType::Gray(_) => ChannelCount::L,
        tiff::ColorType::RGB(_) => ChannelCount::Rgb,
        tiff::ColorType::RGBA(_) => ChannelCount::Rgba,
        color => return Err(Error::UnsupportedColorType(format!("{color:?}"))),
    };
    let sample_type = match decoder.image_buffer_layout()?.sample_type {
        Some(DecodingSampleType::U8) => SampleType::U8,
        Some(DecodingSampleType::U16) => SampleType::U16,
        Some(DecodingSampleType::F32) => SampleType::F32,
        other => {
            return Err(Error::UnsupportedFormat(format!(
                "TIFF sample format not supported: {}",
                sample_format_name(other)
            )));
        }
    };
    let (width, height) = decoder.dimensions()?;
    let desc = ImageDesc::new(
        width as usize,
        height as usize,
        ColorFormat::new(channel_count, sample_type),
    );
    desc.validate()?;
    if !admit(&desc) {
        return Ok(None);
    }
    // Each strip or tile streams through a reader its byte count bounds, so the image is the one
    // allocation a header sizes: the decoder may make no larger one than was admitted.
    let mut limits = Limits::unlimited();
    limits.decoding_buffer_size = desc.size_in_bytes();
    let mut decoder = decoder.with_limits(limits);

    // The decoded samples are handed over whole.
    let image = match decoder.read_image()? {
        DecodingResult::U8(samples) => Image::from_samples(desc, samples),
        DecodingResult::U16(samples) => Image::from_samples(desc, samples),
        DecodingResult::F32(samples) => Image::from_samples(desc, samples),
        _ => unreachable!("a TIFF decodes to the sample type its header names"),
    }?;
    Ok(Some(image))
}

/// The sample type a TIFF header names, or what it names when the codec reads no type of it.
const fn sample_format_name(sample_type: Option<DecodingSampleType>) -> &'static str {
    match sample_type {
        Some(DecodingSampleType::U8) => "u8",
        Some(DecodingSampleType::U16) => "u16",
        Some(DecodingSampleType::U32) => "u32",
        Some(DecodingSampleType::U64) => "u64",
        Some(DecodingSampleType::I8) => "i8",
        Some(DecodingSampleType::I16) => "i16",
        Some(DecodingSampleType::I32) => "i32",
        Some(DecodingSampleType::I64) => "i64",
        Some(DecodingSampleType::F16) => "f16",
        Some(DecodingSampleType::F32) => "f32",
        Some(DecodingSampleType::F64) => "f64",
        None => "a bit depth the codec reads no sample type of",
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
