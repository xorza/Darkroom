//! The raster formats: decoding a file's bytes into an [`InterleavedImage`] and encoding one back.

pub(crate) mod raster_format;

use std::io::{BufRead, Seek, Write};

use bytemuck::Pod;
use image as image_lib;
use tiff::decoder::{Decoder, DecodingResult, Limits};
use tiff::encoder::colortype::{
    ColorType, Gray8, Gray16, Gray32Float, RGB8, RGB16, RGB32Float, RGBA8, RGBA16, RGBA32Float,
};
use tiff::encoder::{TiffEncoder, TiffValue};

use common::USize2;

use crate::codec::raster_format::RasterFormat;
use crate::color::color_format::{ChannelCount, ColorFormat, SampleType};
use crate::color::sample::Sample;
use crate::error::{CodecError, TiffSampleType};
use crate::interleaved::interleaved_desc::InterleavedDesc;
use crate::interleaved::interleaved_image::InterleavedImage;

/// A sink an encoder writes into and seeks back over, as the TIFF and PNG encoders do.
pub(crate) trait WriteSeek: Write + Seek {}

impl<T: Write + Seek> WriteSeek for T {}

/// A source a decoder reads in buffered steps and seeks back over, as the TIFF decoder does.
pub(crate) trait BufReadSeek: BufRead + Seek {}

impl<T: BufRead + Seek> BufReadSeek for T {}

/// The image `input` holds in `format`, refused before its samples are allocated when its header
/// states more than `max_bytes` of them.
pub(crate) fn decode(
    input: &mut dyn BufReadSeek,
    format: RasterFormat,
    max_bytes: u64,
) -> Result<InterleavedImage, CodecError> {
    match format {
        RasterFormat::Png => load_png_jpeg(input, image_lib::ImageFormat::Png, max_bytes),
        RasterFormat::Jpeg => load_png_jpeg(input, image_lib::ImageFormat::Jpeg, max_bytes),
        RasterFormat::Tiff => load_tiff(input, max_bytes),
    }
}

/// `stated` bytes of decoded samples, when they are at most `max_bytes`.
const fn check_stated(stated: u64, max_bytes: u64) -> Result<(), CodecError> {
    if stated > max_bytes {
        return Err(CodecError::Oversized { stated, max_bytes });
    }
    Ok(())
}

pub(crate) fn encode(
    image: &InterleavedImage,
    format: RasterFormat,
    out: &mut dyn WriteSeek,
) -> Result<(), CodecError> {
    match format {
        RasterFormat::Png => save_png(image, out),
        RasterFormat::Jpeg => save_jpg(image, out),
        RasterFormat::Tiff => save_tiff(image, out),
    }
}

fn load_png_jpeg(
    input: &mut dyn BufReadSeek,
    format: image_lib::ImageFormat,
    max_bytes: u64,
) -> Result<InterleavedImage, CodecError> {
    use image_lib::{DynamicImage, ImageDecoder};

    let mut reader = image_lib::ImageReader::with_format(input, format);
    // The caller's cap replaces the codec's default of 512 MiB, which would refuse a large image
    // the machine can hold.
    reader.no_limits();
    let decoder = reader.into_decoder()?;
    check_stated(decoder.total_bytes(), max_bytes)?;
    let img = DynamicImage::from_decoder(decoder)?;
    let size = USize2::new(img.width() as usize, img.height() as usize);
    let desc = |format| InterleavedDesc::checked(size, format);
    // Each decoded buffer is handed over whole. Mono with alpha has no format of its own here;
    // it widens to RGBA, which keeps the alpha.
    match img {
        DynamicImage::ImageLuma8(buffer) => Ok(InterleavedImage::from_samples(
            desc(ColorFormat::MONO_U8)?,
            buffer.into_raw(),
        )),
        DynamicImage::ImageLuma16(buffer) => Ok(InterleavedImage::from_samples(
            desc(ColorFormat::MONO_U16)?,
            buffer.into_raw(),
        )),
        DynamicImage::ImageRgb8(buffer) => Ok(InterleavedImage::from_samples(
            desc(ColorFormat::RGB_U8)?,
            buffer.into_raw(),
        )),
        DynamicImage::ImageRgb16(buffer) => Ok(InterleavedImage::from_samples(
            desc(ColorFormat::RGB_U16)?,
            buffer.into_raw(),
        )),
        DynamicImage::ImageRgb32F(buffer) => Ok(InterleavedImage::from_samples(
            desc(ColorFormat::RGB_F32)?,
            buffer.into_raw(),
        )),
        DynamicImage::ImageRgba8(buffer) => Ok(InterleavedImage::from_samples(
            desc(ColorFormat::RGBA_U8)?,
            buffer.into_raw(),
        )),
        DynamicImage::ImageRgba16(buffer) => Ok(InterleavedImage::from_samples(
            desc(ColorFormat::RGBA_U16)?,
            buffer.into_raw(),
        )),
        DynamicImage::ImageRgba32F(buffer) => Ok(InterleavedImage::from_samples(
            desc(ColorFormat::RGBA_F32)?,
            buffer.into_raw(),
        )),
        DynamicImage::ImageLumaA8(_) => Ok(InterleavedImage::from_samples(
            desc(ColorFormat::RGBA_U8)?,
            img.into_rgba8().into_raw(),
        )),
        DynamicImage::ImageLumaA16(_) => Ok(InterleavedImage::from_samples(
            desc(ColorFormat::RGBA_U16)?,
            img.into_rgba16().into_raw(),
        )),
        img => Err(CodecError::ImageColor(img.color())),
    }
}

fn load_tiff(input: &mut dyn BufReadSeek, max_bytes: u64) -> Result<InterleavedImage, CodecError> {
    // The caller's cap, checked against the header below, replaces the decoder's own.
    let mut decoder = Decoder::new(input)?.with_limits(Limits::unlimited());

    let (channel_count, bits) = match decoder.colortype()? {
        tiff::ColorType::Gray(bits) => (ChannelCount::Mono, bits),
        tiff::ColorType::RGB(bits) => (ChannelCount::Rgb, bits),
        tiff::ColorType::RGBA(bits) => (ChannelCount::Rgba, bits),
        color => return Err(CodecError::TiffColor(color)),
    };
    // A sample of another width is packed, or scaled to no full scale of its type.
    if !matches!(bits, 8 | 16 | 32) {
        return Err(CodecError::TiffBitDepth(bits));
    }
    let (width, height) = decoder.dimensions()?;
    let size = USize2::new(width as usize, height as usize);
    let desc = |sample_type| {
        InterleavedDesc::checked(size, ColorFormat::new(channel_count, sample_type))
            .map_err(CodecError::from)
    };
    // Every sample type this reads is as wide as its bit depth states, so the header alone sets
    // what the decode allocates.
    let stated = size.pixel_count() as u64 * channel_count.count() as u64 * u64::from(bits / 8);
    check_stated(stated, max_bytes)?;

    // The decoded samples are handed over whole, once they are the image's: the decoder reads only
    // the first plane of a planar file, and every sample of a pixel with more than its color type
    // names.
    match decoder.read_image()? {
        DecodingResult::U8(samples) => tiff_image(desc(SampleType::U8)?, samples),
        DecodingResult::U16(samples) => tiff_image(desc(SampleType::U16)?, samples),
        DecodingResult::F32(samples) => tiff_image(desc(SampleType::F32)?, samples),
        DecodingResult::U32(_) => Err(CodecError::TiffSamples(TiffSampleType::U32)),
        DecodingResult::U64(_) => Err(CodecError::TiffSamples(TiffSampleType::U64)),
        DecodingResult::I8(_) => Err(CodecError::TiffSamples(TiffSampleType::I8)),
        DecodingResult::I16(_) => Err(CodecError::TiffSamples(TiffSampleType::I16)),
        DecodingResult::I32(_) => Err(CodecError::TiffSamples(TiffSampleType::I32)),
        DecodingResult::I64(_) => Err(CodecError::TiffSamples(TiffSampleType::I64)),
        DecodingResult::F16(_) => Err(CodecError::TiffSamples(TiffSampleType::F16)),
        DecodingResult::F64(_) => Err(CodecError::TiffSamples(TiffSampleType::F64)),
    }
}

/// The image of `desc` the decoded `samples` are, when they are as many as it has.
fn tiff_image<T: Sample>(
    desc: InterleavedDesc,
    samples: Vec<T>,
) -> Result<InterleavedImage, CodecError> {
    let expected = desc.sample_count();
    if samples.len() != expected {
        return Err(CodecError::TiffLayout {
            desc,
            decoded: samples.len(),
        });
    }
    Ok(InterleavedImage::from_samples(desc, samples))
}

/// The size as the `u32`s the encoders take.
fn encoder_size(image: &InterleavedImage) -> Result<[u32; 2], CodecError> {
    let desc = image.desc();
    let fit = |extent: usize| u32::try_from(extent).map_err(|_| CodecError::TooLarge { desc });
    Ok([fit(desc.size().width)?, fit(desc.size().height)?])
}

fn save_jpg(image: &InterleavedImage, mut out: &mut dyn WriteSeek) -> Result<(), CodecError> {
    let format = image.desc().color_format();
    let color_type = match format {
        ColorFormat::MONO_U8 => image_lib::ColorType::L8,
        ColorFormat::RGB_U8 => image_lib::ColorType::Rgb8,
        _ => {
            return Err(CodecError::Unstorable {
                format: RasterFormat::Jpeg,
                color_format: format,
            });
        }
    };
    let [width, height] = encoder_size(image)?;
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

fn save_png(image: &InterleavedImage, mut out: &mut dyn WriteSeek) -> Result<(), CodecError> {
    let format = image.desc().color_format();
    let color_type = match (format.channel_count, format.sample_type) {
        (ChannelCount::Mono, SampleType::U8) => image_lib::ColorType::L8,
        (ChannelCount::Rgb, SampleType::U8) => image_lib::ColorType::Rgb8,
        (ChannelCount::Rgba, SampleType::U8) => image_lib::ColorType::Rgba8,
        (ChannelCount::Mono, SampleType::U16) => image_lib::ColorType::L16,
        (ChannelCount::Rgb, SampleType::U16) => image_lib::ColorType::Rgb16,
        (ChannelCount::Rgba, SampleType::U16) => image_lib::ColorType::Rgba16,
        (_, SampleType::F32) => {
            return Err(CodecError::Unstorable {
                format: RasterFormat::Png,
                color_format: format,
            });
        }
    };
    let [width, height] = encoder_size(image)?;
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

fn save_tiff(image: &InterleavedImage, out: &mut dyn WriteSeek) -> Result<(), CodecError> {
    let format = image.desc().color_format();
    match (format.channel_count, format.sample_type) {
        (ChannelCount::Mono, SampleType::U8) => write_tiff::<Gray8>(image, out),
        (ChannelCount::Mono, SampleType::U16) => write_tiff::<Gray16>(image, out),
        (ChannelCount::Mono, SampleType::F32) => write_tiff::<Gray32Float>(image, out),
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
fn write_tiff<C>(image: &InterleavedImage, out: &mut dyn WriteSeek) -> Result<(), CodecError>
where
    C: ColorType,
    C::Inner: Pod,
    [C::Inner]: TiffValue,
{
    let samples: &[C::Inner] = bytemuck::cast_slice(image.bytes());
    let [width, height] = encoder_size(image)?;
    let mut tiff = TiffEncoder::new(out)?;
    tiff.new_image::<C>(width, height)?.write_data(samples)?;
    Ok(())
}

#[cfg(feature = "internals")]
pub(crate) mod bench;
#[cfg(test)]
mod tests;
