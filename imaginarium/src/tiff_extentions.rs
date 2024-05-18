use std::fs::File;
use std::mem::{align_of, size_of};

use bytemuck::{Pod, PodCastError};
use tiff::encoder::colortype::*;
use tiff::encoder::{colortype, TiffEncoder, TiffValue};
use tiff::tags::{PhotometricInterpretation, SampleFormat};

use crate::color_format::*;
use crate::image::Image;

pub struct GrayAlphaI8;

impl ColorType for GrayAlphaI8 {
    type Inner = i8;
    const TIFF_VALUE: PhotometricInterpretation = PhotometricInterpretation::BlackIsZero;
    const BITS_PER_SAMPLE: &'static [u16] = &[8, 8];
    const SAMPLE_FORMAT: &'static [SampleFormat] = &[SampleFormat::Int; 2];
}

pub struct GrayAlphaI16;

impl ColorType for GrayAlphaI16 {
    type Inner = i16;
    const TIFF_VALUE: PhotometricInterpretation = PhotometricInterpretation::BlackIsZero;
    const BITS_PER_SAMPLE: &'static [u16] = &[16, 16];
    const SAMPLE_FORMAT: &'static [SampleFormat] = &[SampleFormat::Int; 2];
}

pub struct GrayAlphaI32;

impl ColorType for GrayAlphaI32 {
    type Inner = i32;
    const TIFF_VALUE: PhotometricInterpretation = PhotometricInterpretation::BlackIsZero;
    const BITS_PER_SAMPLE: &'static [u16] = &[32, 32];
    const SAMPLE_FORMAT: &'static [SampleFormat] = &[SampleFormat::Int; 2];
}

pub struct GrayAlphaI64;

impl ColorType for GrayAlphaI64 {
    type Inner = i64;
    const TIFF_VALUE: PhotometricInterpretation = PhotometricInterpretation::BlackIsZero;
    const BITS_PER_SAMPLE: &'static [u16] = &[64, 64];
    const SAMPLE_FORMAT: &'static [SampleFormat] = &[SampleFormat::Int; 2];
}

pub struct GrayAlpha8;

impl ColorType for GrayAlpha8 {
    type Inner = u8;
    const TIFF_VALUE: PhotometricInterpretation = PhotometricInterpretation::BlackIsZero;
    const BITS_PER_SAMPLE: &'static [u16] = &[8, 8];
    const SAMPLE_FORMAT: &'static [SampleFormat] = &[SampleFormat::Uint; 2];
}

pub struct GrayAlpha16;

impl ColorType for GrayAlpha16 {
    type Inner = u16;
    const TIFF_VALUE: PhotometricInterpretation = PhotometricInterpretation::BlackIsZero;
    const BITS_PER_SAMPLE: &'static [u16] = &[16, 16];
    const SAMPLE_FORMAT: &'static [SampleFormat] = &[SampleFormat::Uint; 2];
}

pub struct GrayAlpha32;

impl ColorType for GrayAlpha32 {
    type Inner = u32;
    const TIFF_VALUE: PhotometricInterpretation = PhotometricInterpretation::BlackIsZero;
    const BITS_PER_SAMPLE: &'static [u16] = &[32, 32];
    const SAMPLE_FORMAT: &'static [SampleFormat] = &[SampleFormat::Uint; 2];
}

pub struct GrayAlpha64;

impl ColorType for GrayAlpha64 {
    type Inner = u64;
    const TIFF_VALUE: PhotometricInterpretation = PhotometricInterpretation::BlackIsZero;
    const BITS_PER_SAMPLE: &'static [u16] = &[64, 64];
    const SAMPLE_FORMAT: &'static [SampleFormat] = &[SampleFormat::Uint; 2];
}

pub struct GrayAlpha32Float;

impl ColorType for GrayAlpha32Float {
    type Inner = f32;
    const TIFF_VALUE: PhotometricInterpretation = PhotometricInterpretation::BlackIsZero;
    const BITS_PER_SAMPLE: &'static [u16] = &[32, 32];
    const SAMPLE_FORMAT: &'static [SampleFormat] = &[SampleFormat::IEEEFP; 2];
}

pub struct GrayAlpha64Float;

impl ColorType for GrayAlpha64Float {
    type Inner = f64;
    const TIFF_VALUE: PhotometricInterpretation = PhotometricInterpretation::BlackIsZero;
    const BITS_PER_SAMPLE: &'static [u16] = &[64, 64];
    const SAMPLE_FORMAT: &'static [SampleFormat] = &[SampleFormat::IEEEFP; 2];
}

pub struct RGBI8;

impl ColorType for RGBI8 {
    type Inner = i8;
    const TIFF_VALUE: PhotometricInterpretation = PhotometricInterpretation::RGB;
    const BITS_PER_SAMPLE: &'static [u16] = &[8, 8, 8];
    const SAMPLE_FORMAT: &'static [SampleFormat] = &[SampleFormat::Int; 3];
}

pub struct RGBI16;

impl ColorType for RGBI16 {
    type Inner = i16;
    const TIFF_VALUE: PhotometricInterpretation = PhotometricInterpretation::RGB;
    const BITS_PER_SAMPLE: &'static [u16] = &[16, 16, 16];
    const SAMPLE_FORMAT: &'static [SampleFormat] = &[SampleFormat::Int; 3];
}

pub struct RGBI32;

impl ColorType for RGBI32 {
    type Inner = i32;
    const TIFF_VALUE: PhotometricInterpretation = PhotometricInterpretation::RGB;
    const BITS_PER_SAMPLE: &'static [u16] = &[32, 32, 32];
    const SAMPLE_FORMAT: &'static [SampleFormat] = &[SampleFormat::Int; 3];
}

pub struct RGBI64;

impl ColorType for RGBI64 {
    type Inner = i64;
    const TIFF_VALUE: PhotometricInterpretation = PhotometricInterpretation::RGB;
    const BITS_PER_SAMPLE: &'static [u16] = &[64, 64, 64];
    const SAMPLE_FORMAT: &'static [SampleFormat] = &[SampleFormat::Int; 3];
}

pub struct RGBAI8;

impl ColorType for RGBAI8 {
    type Inner = i8;
    const TIFF_VALUE: PhotometricInterpretation = PhotometricInterpretation::RGB;
    const BITS_PER_SAMPLE: &'static [u16] = &[8, 8, 8];
    const SAMPLE_FORMAT: &'static [SampleFormat] = &[SampleFormat::Int; 4];
}

pub struct RGBAI16;

impl ColorType for RGBAI16 {
    type Inner = i16;
    const TIFF_VALUE: PhotometricInterpretation = PhotometricInterpretation::RGB;
    const BITS_PER_SAMPLE: &'static [u16] = &[16, 16, 16];
    const SAMPLE_FORMAT: &'static [SampleFormat] = &[SampleFormat::Int; 4];
}

pub struct RGBAI32;

impl ColorType for RGBAI32 {
    type Inner = i32;
    const TIFF_VALUE: PhotometricInterpretation = PhotometricInterpretation::RGB;
    const BITS_PER_SAMPLE: &'static [u16] = &[32, 32, 32];
    const SAMPLE_FORMAT: &'static [SampleFormat] = &[SampleFormat::Int; 4];
}

pub struct RGBAI64;

impl ColorType for RGBAI64 {
    type Inner = i64;
    const TIFF_VALUE: PhotometricInterpretation = PhotometricInterpretation::RGB;
    const BITS_PER_SAMPLE: &'static [u16] = &[64, 64, 64];
    const SAMPLE_FORMAT: &'static [SampleFormat] = &[SampleFormat::Int; 4];
}

pub(crate) fn save_tiff(image: &Image, filename: &str) -> anyhow::Result<()> {
    match (
        image.desc.color_format().channel_count,
        image.desc.color_format().channel_size,
        image.desc.color_format().channel_type,
    ) {
        // @formatter:off
        (ChannelCount::Gray, ChannelSize::_8bit, ChannelType::Int) => {
            save_tiff_internal::<GrayI8>(image, filename)?
        }
        (ChannelCount::Gray, ChannelSize::_16bit, ChannelType::Int) => {
            save_tiff_internal::<GrayI16>(image, filename)?
        }
        (ChannelCount::Gray, ChannelSize::_32bit, ChannelType::Int) => {
            save_tiff_internal::<GrayI32>(image, filename)?
        }
        (ChannelCount::Gray, ChannelSize::_64bit, ChannelType::Int) => {
            save_tiff_internal::<GrayI64>(image, filename)?
        }

        (ChannelCount::Gray, ChannelSize::_8bit, ChannelType::UInt) => {
            save_tiff_internal::<Gray8>(image, filename)?
        }
        (ChannelCount::Gray, ChannelSize::_16bit, ChannelType::UInt) => {
            save_tiff_internal::<Gray16>(image, filename)?
        }
        (ChannelCount::Gray, ChannelSize::_32bit, ChannelType::UInt) => {
            save_tiff_internal::<Gray32>(image, filename)?
        }
        (ChannelCount::Gray, ChannelSize::_64bit, ChannelType::UInt) => {
            save_tiff_internal::<Gray64>(image, filename)?
        }

        (ChannelCount::Gray, ChannelSize::_32bit, ChannelType::Float) => {
            save_tiff_internal::<Gray32Float>(image, filename)?
        }
        (ChannelCount::Gray, ChannelSize::_64bit, ChannelType::Float) => {
            save_tiff_internal::<Gray64Float>(image, filename)?
        }

        (ChannelCount::GrayAlpha, ChannelSize::_8bit, ChannelType::Int) => {
            save_tiff_internal::<GrayAlphaI8>(image, filename)?
        }
        (ChannelCount::GrayAlpha, ChannelSize::_16bit, ChannelType::Int) => {
            save_tiff_internal::<GrayAlphaI16>(image, filename)?
        }
        (ChannelCount::GrayAlpha, ChannelSize::_32bit, ChannelType::Int) => {
            save_tiff_internal::<GrayAlphaI32>(image, filename)?
        }
        (ChannelCount::GrayAlpha, ChannelSize::_64bit, ChannelType::Int) => {
            save_tiff_internal::<GrayAlphaI64>(image, filename)?
        }

        (ChannelCount::GrayAlpha, ChannelSize::_8bit, ChannelType::UInt) => {
            save_tiff_internal::<GrayAlpha8>(image, filename)?
        }
        (ChannelCount::GrayAlpha, ChannelSize::_16bit, ChannelType::UInt) => {
            save_tiff_internal::<GrayAlpha16>(image, filename)?
        }
        (ChannelCount::GrayAlpha, ChannelSize::_32bit, ChannelType::UInt) => {
            save_tiff_internal::<GrayAlpha32>(image, filename)?
        }
        (ChannelCount::GrayAlpha, ChannelSize::_64bit, ChannelType::UInt) => {
            save_tiff_internal::<GrayAlpha64>(image, filename)?
        }

        (ChannelCount::GrayAlpha, ChannelSize::_32bit, ChannelType::Float) => {
            save_tiff_internal::<GrayAlpha32Float>(image, filename)?
        }
        (ChannelCount::GrayAlpha, ChannelSize::_64bit, ChannelType::Float) => {
            save_tiff_internal::<GrayAlpha64Float>(image, filename)?
        }

        (ChannelCount::Rgb, ChannelSize::_8bit, ChannelType::Int) => {
            save_tiff_internal::<RGBI8>(image, filename)?
        }
        (ChannelCount::Rgb, ChannelSize::_16bit, ChannelType::Int) => {
            save_tiff_internal::<RGBI16>(image, filename)?
        }
        (ChannelCount::Rgb, ChannelSize::_32bit, ChannelType::Int) => {
            save_tiff_internal::<RGBI32>(image, filename)?
        }
        (ChannelCount::Rgb, ChannelSize::_64bit, ChannelType::Int) => {
            save_tiff_internal::<RGBI64>(image, filename)?
        }

        (ChannelCount::Rgb, ChannelSize::_8bit, ChannelType::UInt) => {
            save_tiff_internal::<RGB8>(image, filename)?
        }
        (ChannelCount::Rgb, ChannelSize::_16bit, ChannelType::UInt) => {
            save_tiff_internal::<RGB16>(image, filename)?
        }
        (ChannelCount::Rgb, ChannelSize::_32bit, ChannelType::UInt) => {
            save_tiff_internal::<RGB32>(image, filename)?
        }
        (ChannelCount::Rgb, ChannelSize::_64bit, ChannelType::UInt) => {
            save_tiff_internal::<RGB64>(image, filename)?
        }

        (ChannelCount::Rgb, ChannelSize::_32bit, ChannelType::Float) => {
            save_tiff_internal::<RGB32Float>(image, filename)?
        }
        (ChannelCount::Rgb, ChannelSize::_64bit, ChannelType::Float) => {
            save_tiff_internal::<RGB64Float>(image, filename)?
        }

        (ChannelCount::Rgba, ChannelSize::_8bit, ChannelType::Int) => {
            save_tiff_internal::<RGBAI8>(image, filename)?
        }
        (ChannelCount::Rgba, ChannelSize::_16bit, ChannelType::Int) => {
            save_tiff_internal::<RGBAI16>(image, filename)?
        }
        (ChannelCount::Rgba, ChannelSize::_32bit, ChannelType::Int) => {
            save_tiff_internal::<RGBAI32>(image, filename)?
        }
        (ChannelCount::Rgba, ChannelSize::_64bit, ChannelType::Int) => {
            save_tiff_internal::<RGBAI64>(image, filename)?
        }

        (ChannelCount::Rgba, ChannelSize::_8bit, ChannelType::UInt) => {
            save_tiff_internal::<RGBA8>(image, filename)?
        }
        (ChannelCount::Rgba, ChannelSize::_16bit, ChannelType::UInt) => {
            save_tiff_internal::<RGBA16>(image, filename)?
        }
        (ChannelCount::Rgba, ChannelSize::_32bit, ChannelType::UInt) => {
            save_tiff_internal::<RGBA32>(image, filename)?
        }
        (ChannelCount::Rgba, ChannelSize::_64bit, ChannelType::UInt) => {
            save_tiff_internal::<RGBA64>(image, filename)?
        }

        (ChannelCount::Rgba, ChannelSize::_32bit, ChannelType::Float) => {
            save_tiff_internal::<RGBA32Float>(image, filename)?
        }
        (ChannelCount::Rgba, ChannelSize::_64bit, ChannelType::Float) => {
            save_tiff_internal::<RGBA64Float>(image, filename)?
        }

        // @formatter:on
        (_, _, _) => {
            return Err(anyhow::anyhow!(
                "Unsupported TIFF format: {:?} {:?} {:?}",
                image.desc.color_format().channel_count,
                image.desc.color_format().channel_size,
                image.desc.color_format().channel_type
            ))
        }
    };

    Ok(())
}

fn save_tiff_internal<ColorType>(image: &Image, filename: &str) -> anyhow::Result<()>
where
    ColorType: colortype::ColorType,
    [ColorType::Inner]: TiffValue,
{
    let buf: &[ColorType::Inner] = cast_slice(&image.bytes).unwrap();

    let mut file = File::create(filename)?;
    let mut tiff = TiffEncoder::new(&mut file)?;
    let img = tiff.new_image::<ColorType>(image.desc.width(), image.desc.height())?;

    img.write_data(buf).unwrap();

    Ok(())
}

fn cast_slice<A, B>(a: &[A]) -> Result<&[B], PodCastError>
where
    A: Pod + Copy,
    [B]: TiffValue,
{
    if align_of::<B>() > align_of::<A>() && (a.as_ptr() as usize) % align_of::<B>() != 0 {
        Err(PodCastError::TargetAlignmentGreaterAndInputNotAligned)
    } else if size_of::<B>() == size_of::<A>() {
        Ok(unsafe { core::slice::from_raw_parts(a.as_ptr() as *const B, a.len()) })
    } else if size_of::<A>() == 0 || size_of::<B>() == 0 {
        Err(PodCastError::SizeMismatch)
    } else if core::mem::size_of_val(a) % size_of::<B>() == 0 {
        let new_len = core::mem::size_of_val(a) / size_of::<B>();
        Ok(unsafe { core::slice::from_raw_parts(a.as_ptr() as *const B, new_len) })
    } else {
        Err(PodCastError::OutputSliceWouldHaveSlop)
    }
}
