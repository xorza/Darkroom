use std::fs::File;
use std::path::Path;

use bytemuck::Pod;
use tiff::encoder::colortype::*;
use tiff::encoder::{TiffEncoder, TiffValue, colortype};
use tiff::tags::{PhotometricInterpretation, SampleFormat};

use crate::prelude::*;

macro_rules! define_int_color_type {
    ($name:ident, $inner:ty, $photometric:expr, $bits:expr, $channels:expr) => {
        struct $name;

        impl ColorType for $name {
            type Inner = $inner;
            const TIFF_VALUE: PhotometricInterpretation = $photometric;
            const BITS_PER_SAMPLE: &'static [u16] = &[$bits; $channels];
            const SAMPLE_FORMAT: &'static [SampleFormat] = &[SampleFormat::Uint; $channels];

            fn horizontal_predict(row: &[Self::Inner], result: &mut Vec<Self::Inner>) {
                let sample_size = Self::SAMPLE_FORMAT.len();

                if row.len() < sample_size {
                    debug_assert!(false);
                    return;
                }

                let (start, rest) = row.split_at(sample_size);

                result.extend_from_slice(start);
                if result.capacity() - result.len() < rest.len() {
                    return;
                }

                result.extend(
                    row.iter()
                        .zip(rest)
                        .map(|(prev, current)| current.wrapping_sub(*prev)),
                );
            }
        }
    };
}

macro_rules! define_float_color_type {
    ($name:ident, $inner:ty, $photometric:expr, $bits:expr, $channels:expr) => {
        struct $name;

        impl ColorType for $name {
            type Inner = $inner;
            const TIFF_VALUE: PhotometricInterpretation = $photometric;
            const BITS_PER_SAMPLE: &'static [u16] = &[$bits; $channels];
            const SAMPLE_FORMAT: &'static [SampleFormat] = &[SampleFormat::IEEEFP; $channels];

            fn horizontal_predict(_: &[Self::Inner], _: &mut Vec<Self::Inner>) {
                unreachable!()
            }
        }
    };
}

// Gray unsigned integer types
define_int_color_type!(Gray8U, u8, PhotometricInterpretation::BlackIsZero, 8, 1);
define_int_color_type!(Gray16U, u16, PhotometricInterpretation::BlackIsZero, 16, 1);
define_int_color_type!(Gray32U, u32, PhotometricInterpretation::BlackIsZero, 32, 1);

// Gray float type
define_float_color_type!(Gray32F, f32, PhotometricInterpretation::BlackIsZero, 32, 1);

// GrayAlpha unsigned integer types
define_int_color_type!(
    GrayAlpha8U,
    u8,
    PhotometricInterpretation::BlackIsZero,
    8,
    2
);
define_int_color_type!(
    GrayAlpha16U,
    u16,
    PhotometricInterpretation::BlackIsZero,
    16,
    2
);
define_int_color_type!(
    GrayAlpha32U,
    u32,
    PhotometricInterpretation::BlackIsZero,
    32,
    2
);

// GrayAlpha float type
define_float_color_type!(
    GrayAlpha32F,
    f32,
    PhotometricInterpretation::BlackIsZero,
    32,
    2
);

// RGB unsigned integer types
define_int_color_type!(RGB8U, u8, PhotometricInterpretation::RGB, 8, 3);
define_int_color_type!(RGB16U, u16, PhotometricInterpretation::RGB, 16, 3);
define_int_color_type!(RGB32U, u32, PhotometricInterpretation::RGB, 32, 3);

// RGB float type
define_float_color_type!(RGB32F, f32, PhotometricInterpretation::RGB, 32, 3);

// RGBA unsigned integer types
define_int_color_type!(RGBA8U, u8, PhotometricInterpretation::RGB, 8, 4);
define_int_color_type!(RGBA16U, u16, PhotometricInterpretation::RGB, 16, 4);
define_int_color_type!(RGBA32U, u32, PhotometricInterpretation::RGB, 32, 4);

// RGBA float type
define_float_color_type!(RGBA32F, f32, PhotometricInterpretation::RGB, 32, 4);

macro_rules! dispatch_tiff {
    ($image:expr, $filename:expr, {
        $( ($count:ident, $size:ident, $type:ident) => $color_type:ty ),+ $(,)?
    }) => {
        match (
            $image.desc.color_format.channel_count,
            $image.desc.color_format.channel_size,
            $image.desc.color_format.channel_type,
        ) {
            $(
                (ChannelCount::$count, ChannelSize::$size, ChannelType::$type) => {
                    save_tiff_internal::<$color_type, _>($image, $filename)?
                }
            )+
            (_, _, _) => {
                return Err(Error::UnsupportedFormat(format!(
                    "TIFF format: {:?} {:?} {:?}",
                    $image.desc.color_format.channel_count,
                    $image.desc.color_format.channel_size,
                    $image.desc.color_format.channel_type
                )));
            }
        }
    };
}

pub(crate) fn save_tiff<P: AsRef<Path>>(image: &Image, filename: P) -> Result<()> {
    dispatch_tiff!(image, filename, {
        // Gray
        (Gray, _8bit, UInt) => Gray8U,
        (Gray, _16bit, UInt) => Gray16U,
        (Gray, _32bit, UInt) => Gray32U,
        (Gray, _32bit, Float) => Gray32F,
        // GrayAlpha
        (GrayAlpha, _8bit, UInt) => GrayAlpha8U,
        (GrayAlpha, _16bit, UInt) => GrayAlpha16U,
        (GrayAlpha, _32bit, UInt) => GrayAlpha32U,
        (GrayAlpha, _32bit, Float) => GrayAlpha32F,
        // RGB
        (Rgb, _8bit, UInt) => RGB8U,
        (Rgb, _16bit, UInt) => RGB16U,
        (Rgb, _32bit, UInt) => RGB32U,
        (Rgb, _32bit, Float) => RGB32F,
        // RGBA
        (Rgba, _8bit, UInt) => RGBA8U,
        (Rgba, _16bit, UInt) => RGBA16U,
        (Rgba, _32bit, UInt) => RGBA32U,
        (Rgba, _32bit, Float) => RGBA32F,
    });

    Ok(())
}

fn save_tiff_internal<CT, P: AsRef<Path>>(image: &Image, filename: P) -> Result<()>
where
    CT: colortype::ColorType,
    CT::Inner: Pod,
    [CT::Inner]: TiffValue,
{
    let buf: &[CT::Inner] = bytemuck::try_cast_slice(image.bytes())?;

    let mut file = File::create(filename)?;
    let mut tiff = TiffEncoder::new(&mut file)?;
    let img = tiff.new_image::<CT>(image.desc().width, image.desc().height)?;

    img.write_data(buf)?;

    Ok(())
}
