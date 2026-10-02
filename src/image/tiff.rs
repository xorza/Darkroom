use std::fs::File;
use std::path::Path;

use bytemuck::Pod;
use tiff::encoder::colortype::{
    ColorType, Gray8, Gray16, Gray32Float, RGB8, RGB16, RGB32Float, RGBA8, RGBA16, RGBA32Float,
};
use tiff::encoder::{TiffEncoder, TiffValue};

use crate::common::color_format::{ChannelCount, SampleType};
use crate::common::error::Result;
use crate::image::{Image, io};

pub(super) fn save_tiff(image: &Image, filename: &Path) -> Result<()> {
    let format = image.desc().color_format;
    match (format.channel_count, format.sample_type) {
        (ChannelCount::L, SampleType::U8) => write::<Gray8>(image, filename),
        (ChannelCount::L, SampleType::U16) => write::<Gray16>(image, filename),
        (ChannelCount::L, SampleType::F32) => write::<Gray32Float>(image, filename),
        (ChannelCount::Rgb, SampleType::U8) => write::<RGB8>(image, filename),
        (ChannelCount::Rgb, SampleType::U16) => write::<RGB16>(image, filename),
        (ChannelCount::Rgb, SampleType::F32) => write::<RGB32Float>(image, filename),
        (ChannelCount::Rgba, SampleType::U8) => write::<RGBA8>(image, filename),
        (ChannelCount::Rgba, SampleType::U16) => write::<RGBA16>(image, filename),
        (ChannelCount::Rgba, SampleType::F32) => write::<RGBA32Float>(image, filename),
    }
}

/// Writes `image` as the TIFF colour type `C`, whose sample type the caller matched to the
/// image's format — so the cast of the image's own aligned storage cannot fail.
fn write<C>(image: &Image, filename: &Path) -> Result<()>
where
    C: ColorType,
    C::Inner: Pod,
    [C::Inner]: TiffValue,
{
    let samples: &[C::Inner] = bytemuck::cast_slice(image.bytes());
    let [width, height] = io::encoder_dimensions(image)?;
    let mut file = File::create(filename)?;
    let mut tiff = TiffEncoder::new(&mut file)?;
    tiff.new_image::<C>(width, height)?.write_data(samples)?;
    Ok(())
}
