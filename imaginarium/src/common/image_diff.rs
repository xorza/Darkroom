//! Image comparison utilities for testing.

use rayon::prelude::*;

use crate::common::color_format::{ChannelSize, ChannelType};
use crate::image::Image;

/// Computes the maximum per-channel difference between two images.
/// Returns the difference normalized to [0, 1] range for integer types,
/// or absolute difference for float types.
///
/// Only compares actual pixel data, ignoring stride padding.
///
/// # Panics
/// Panics if images have different dimensions or formats.
pub fn max_pixel_diff(img1: &Image, img2: &Image) -> f64 {
    assert_eq!(img1.desc().width, img2.desc().width, "width mismatch");
    assert_eq!(img1.desc().height, img2.desc().height, "height mismatch");
    assert_eq!(
        img1.desc().color_format,
        img2.desc().color_format,
        "format mismatch"
    );

    let width = img1.desc().width as usize;
    let height = img1.desc().height as usize;
    let format = img1.desc().color_format;
    let pixel_size = format.byte_count() as usize;
    let row_bytes = width * pixel_size;
    let stride1 = img1.desc().stride;
    let stride2 = img2.desc().stride;

    (0..height)
        .into_par_iter()
        .map(|y| {
            let row1 = &img1.bytes()[y * stride1..y * stride1 + row_bytes];
            let row2 = &img2.bytes()[y * stride2..y * stride2 + row_bytes];
            row_max_diff(row1, row2, format.channel_size, format.channel_type)
        })
        .reduce(|| 0.0, f64::max)
}

/// Computes the maximum difference for a single row of pixel data.
fn row_max_diff(row1: &[u8], row2: &[u8], size: ChannelSize, typ: ChannelType) -> f64 {
    match (size, typ) {
        (ChannelSize::_8bit, ChannelType::UInt) => row1
            .iter()
            .zip(row2.iter())
            .map(|(a, b)| (*a as i32 - *b as i32).unsigned_abs() as f64 / u8::MAX as f64)
            .fold(0.0, f64::max),
        (ChannelSize::_16bit, ChannelType::UInt) => {
            let v1: &[u16] = bytemuck::cast_slice(row1);
            let v2: &[u16] = bytemuck::cast_slice(row2);
            v1.iter()
                .zip(v2.iter())
                .map(|(a, b)| (*a as i32 - *b as i32).unsigned_abs() as f64 / u16::MAX as f64)
                .fold(0.0, f64::max)
        }
        (ChannelSize::_32bit, ChannelType::UInt) => {
            let v1: &[u32] = bytemuck::cast_slice(row1);
            let v2: &[u32] = bytemuck::cast_slice(row2);
            v1.iter()
                .zip(v2.iter())
                .map(|(a, b)| (*a as i64 - *b as i64).unsigned_abs() as f64 / u32::MAX as f64)
                .fold(0.0, f64::max)
        }
        (ChannelSize::_32bit, ChannelType::Float) => {
            let v1: &[f32] = bytemuck::cast_slice(row1);
            let v2: &[f32] = bytemuck::cast_slice(row2);
            v1.iter()
                .zip(v2.iter())
                .map(|(a, b)| (a - b).abs() as f64)
                .fold(0.0, f64::max)
        }
        (ChannelSize::_8bit, ChannelType::Float) | (ChannelSize::_16bit, ChannelType::Float) => {
            unreachable!("8-bit and 16-bit float are not valid formats")
        }
    }
}

/// Checks if two images have identical pixel data (ignoring stride padding).
///
/// # Panics
/// Panics if images have different dimensions or formats.
pub fn pixels_equal(img1: &Image, img2: &Image) -> bool {
    assert_eq!(img1.desc().width, img2.desc().width, "width mismatch");
    assert_eq!(img1.desc().height, img2.desc().height, "height mismatch");
    assert_eq!(
        img1.desc().color_format,
        img2.desc().color_format,
        "format mismatch"
    );

    let width = img1.desc().width as usize;
    let height = img1.desc().height as usize;
    let pixel_size = img1.desc().color_format.byte_count() as usize;
    let row_bytes = width * pixel_size;
    let stride1 = img1.desc().stride;
    let stride2 = img2.desc().stride;

    (0..height).into_par_iter().all(|y| {
        let row1 = &img1.bytes()[y * stride1..y * stride1 + row_bytes];
        let row2 = &img2.bytes()[y * stride2..y * stride2 + row_bytes];
        row1 == row2
    })
}
