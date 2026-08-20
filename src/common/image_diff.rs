//! Image comparison utilities for testing.

use bytemuck::Pod;
use rayon::prelude::*;

use crate::common::color_format::{ChannelSize, ChannelType};
use crate::image::Image;

/// The largest per-channel difference between two images: normalized to `[0, 1]`
/// for the integer formats, absolute for float.
///
/// Both differences are taken in `f64`, so an `f32` pair never loses precision
/// to a rounded subtraction before the comparison.
///
/// # Panics
/// Panics unless the two images share a descriptor.
pub(crate) fn max_pixel_diff(img1: &Image, img2: &Image) -> f64 {
    img1.desc().assert_same(img2.desc(), "img1/img2");

    // Pixel data is tightly packed, so the two buffers are one flat channel
    // array each — there is no per-row padding to step over.
    let format = img1.desc().color_format;
    let (a, b) = (img1.bytes(), img2.bytes());
    match (format.channel_size, format.channel_type) {
        (ChannelSize::_8bit, ChannelType::UInt) => max_diff::<u8>(a, b, f64::from(u8::MAX)),
        (ChannelSize::_16bit, ChannelType::UInt) => max_diff::<u16>(a, b, f64::from(u16::MAX)),
        (ChannelSize::_32bit, ChannelType::Float) => max_diff::<f32>(a, b, 1.0),
        _ => unreachable!("unsupported color format: {format:?}"),
    }
}

/// The largest `|a - b| / scale` over two buffers read as `T` channel values.
fn max_diff<T>(a: &[u8], b: &[u8], scale: f64) -> f64
where
    T: Pod + Sync + Into<f64>,
{
    let a: &[T] = bytemuck::cast_slice(a);
    let b: &[T] = bytemuck::cast_slice(b);
    a.par_iter()
        .zip(b.par_iter())
        .map(|(&a, &b)| (a.into() - b.into()).abs() / scale)
        .reduce(|| 0.0, f64::max)
}

/// Whether two images hold byte-identical pixel data.
///
/// # Panics
/// Panics unless the two images share a descriptor.
pub(crate) fn pixels_equal(img1: &Image, img2: &Image) -> bool {
    img1.desc().assert_same(img2.desc(), "img1/img2");
    img1.bytes() == img2.bytes()
}
