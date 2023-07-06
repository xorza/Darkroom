use std::mem::size_of;

use bytemuck::Pod;
use num_traits::Bounded;

use crate::color_format::*;
use crate::image::Image;

pub(crate) fn convert_image(from: &Image, to: &mut Image) -> anyhow::Result<()> {
    match (from.info.color_format.channel_size, to.info.color_format.channel_size) {
        // @formatter:off
        (ChannelSize:: _8bit, ChannelSize:: _8bit) =>
            match (from.info.color_format.channel_type, to.info.color_format.channel_type) {
                (ChannelType::  Int, ChannelType::  Int) => convert_pixels::<i8 , i8 >(from, to,  i8_to_i8 , avg_i8 ),
                (ChannelType::  Int, ChannelType:: UInt) => convert_pixels::<i8 , u8 >(from, to,  i8_to_u8 , avg_i8 ),
                (ChannelType:: UInt, ChannelType::  Int) => convert_pixels::<u8 , i8 >(from, to,  u8_to_i8 , avg_u8 ),
                (ChannelType:: UInt, ChannelType:: UInt) => convert_pixels::<u8 , u8 >(from, to,  u8_to_u8 , avg_u8 ),
                (_, _) => return Err(anyhow::anyhow!("Invalid channel type")),
            }
        (ChannelSize:: _8bit, ChannelSize::_16bit) =>
            match (from.info.color_format.channel_type, to.info.color_format.channel_type) {
                (ChannelType::  Int, ChannelType::  Int) => convert_pixels::<i8 , i16>(from, to,  i8_to_i16, avg_i8 ),
                (ChannelType::  Int, ChannelType:: UInt) => convert_pixels::<i8 , u16>(from, to,  i8_to_u16, avg_i8 ),
                (ChannelType:: UInt, ChannelType::  Int) => convert_pixels::<u8 , i16>(from, to,  u8_to_i16, avg_u8 ),
                (ChannelType:: UInt, ChannelType:: UInt) => convert_pixels::<u8 , u16>(from, to,  u8_to_u16, avg_u8 ),
                (_, _) => return Err(anyhow::anyhow!("Invalid channel type")),
            }
        (ChannelSize:: _8bit, ChannelSize::_32bit) =>
            match (from.info.color_format.channel_type, to.info.color_format.channel_type) {
                (ChannelType::  Int, ChannelType::  Int) => convert_pixels::<i8 , i32>(from, to,  i8_to_i32, avg_i8 ),
                (ChannelType::  Int, ChannelType:: UInt) => convert_pixels::<i8 , u32>(from, to,  i8_to_u32, avg_i8 ),
                (ChannelType::  Int, ChannelType::Float) => convert_pixels::<i8 , f32>(from, to,  i8_to_f32, avg_i8 ),
                (ChannelType:: UInt, ChannelType::  Int) => convert_pixels::<u8 , i32>(from, to,  u8_to_i32, avg_u8 ),
                (ChannelType:: UInt, ChannelType:: UInt) => convert_pixels::<u8 , u32>(from, to,  u8_to_u32, avg_u8 ),
                (ChannelType:: UInt, ChannelType::Float) => convert_pixels::<u8 , f32>(from, to,  u8_to_f32, avg_u8 ),
                (_, _) => return Err(anyhow::anyhow!("Invalid channel type")),
            }
        (ChannelSize:: _8bit, ChannelSize::_64bit) =>
            match (from.info.color_format.channel_type, to.info.color_format.channel_type) {
                (ChannelType::  Int, ChannelType::  Int) => convert_pixels::<i8 , i64>(from, to,  i8_to_i64, avg_i8 ),
                (ChannelType::  Int, ChannelType:: UInt) => convert_pixels::<i8 , u64>(from, to,  i8_to_u64, avg_i8 ),
                (ChannelType::  Int, ChannelType::Float) => convert_pixels::<i8 , f64>(from, to,  i8_to_f64, avg_i8 ),
                (ChannelType:: UInt, ChannelType::  Int) => convert_pixels::<u8 , i64>(from, to,  u8_to_i64, avg_u8 ),
                (ChannelType:: UInt, ChannelType:: UInt) => convert_pixels::<u8 , u64>(from, to,  u8_to_u64, avg_u8 ),
                (ChannelType:: UInt, ChannelType::Float) => convert_pixels::<u8 , f64>(from, to,  u8_to_f64, avg_u8 ),
                (_, _) => return Err(anyhow::anyhow!("Invalid channel type")),
            }
        (ChannelSize::_16bit, ChannelSize:: _8bit) =>
            match (from.info.color_format.channel_type, to.info.color_format.channel_type) {
                (ChannelType::  Int, ChannelType::  Int) => convert_pixels::<i16, i8 >(from, to, i16_to_i8 , avg_i16),
                (ChannelType::  Int, ChannelType:: UInt) => convert_pixels::<i16, u8 >(from, to, i16_to_u8 , avg_i16),
                (ChannelType:: UInt, ChannelType::  Int) => convert_pixels::<u16, i8 >(from, to, u16_to_i8 , avg_u16),
                (ChannelType:: UInt, ChannelType:: UInt) => convert_pixels::<u16, u8 >(from, to, u16_to_u8 , avg_u16),
                (_, _) => return Err(anyhow::anyhow!("Invalid channel type")),
            }
        (ChannelSize::_16bit, ChannelSize::_16bit) =>
            match (from.info.color_format.channel_type, to.info.color_format.channel_type) {
                (ChannelType::  Int, ChannelType::  Int) => convert_pixels::<i16, i16>(from, to, i16_to_i16, avg_i16),
                (ChannelType::  Int, ChannelType:: UInt) => convert_pixels::<i16, u16>(from, to, i16_to_u16, avg_i16),
                (ChannelType:: UInt, ChannelType::  Int) => convert_pixels::<u16, i16>(from, to, u16_to_i16, avg_u16),
                (ChannelType:: UInt, ChannelType:: UInt) => convert_pixels::<u16, u16>(from, to, u16_to_u16, avg_u16),
                (_, _) => return Err(anyhow::anyhow!("Invalid channel type")),
            }
        (ChannelSize::_16bit, ChannelSize::_32bit) =>
            match (from.info.color_format.channel_type, to.info.color_format.channel_type) {
                (ChannelType::  Int, ChannelType::  Int) => convert_pixels::<i16, i32>(from, to, i16_to_i32, avg_i16),
                (ChannelType::  Int, ChannelType:: UInt) => convert_pixels::<i16, u32>(from, to, i16_to_u32, avg_i16),
                (ChannelType::  Int, ChannelType::Float) => convert_pixels::<i16, f32>(from, to, i16_to_f32, avg_i16),
                (ChannelType:: UInt, ChannelType::  Int) => convert_pixels::<u16, i32>(from, to, u16_to_i32, avg_u16),
                (ChannelType:: UInt, ChannelType:: UInt) => convert_pixels::<u16, u32>(from, to, u16_to_u32, avg_u16),
                (ChannelType:: UInt, ChannelType::Float) => convert_pixels::<u16, f32>(from, to, u16_to_f32, avg_u16),
                (_, _) => return Err(anyhow::anyhow!("Invalid channel type")),
            }
        (ChannelSize::_16bit, ChannelSize::_64bit) =>
            match (from.info.color_format.channel_type, to.info.color_format.channel_type) {
                (ChannelType::  Int, ChannelType::  Int) => convert_pixels::<i16, i64>(from, to, i16_to_i64, avg_i16),
                (ChannelType::  Int, ChannelType:: UInt) => convert_pixels::<i16, u64>(from, to, i16_to_u64, avg_i16),
                (ChannelType::  Int, ChannelType::Float) => convert_pixels::<i16, f64>(from, to, i16_to_f64, avg_i16),
                (ChannelType:: UInt, ChannelType::  Int) => convert_pixels::<u16, i64>(from, to, u16_to_i64, avg_u16),
                (ChannelType:: UInt, ChannelType:: UInt) => convert_pixels::<u16, u64>(from, to, u16_to_u64, avg_u16),
                (ChannelType:: UInt, ChannelType::Float) => convert_pixels::<u16, f64>(from, to, u16_to_f64, avg_u16),
                (_, _) => return Err(anyhow::anyhow!("Invalid channel type")),
            }
        (ChannelSize::_32bit, ChannelSize:: _8bit) =>
            match (from.info.color_format.channel_type, to.info.color_format.channel_type) {
                (ChannelType::  Int, ChannelType::  Int) => convert_pixels::<i32, i8 >(from, to, i32_to_i8 , avg_i32),
                (ChannelType::  Int, ChannelType:: UInt) => convert_pixels::<i32, u8 >(from, to, i32_to_u8 , avg_i32),
                (ChannelType:: UInt, ChannelType::  Int) => convert_pixels::<u32, i8 >(from, to, u32_to_i8 , avg_u32),
                (ChannelType:: UInt, ChannelType:: UInt) => convert_pixels::<u32, u8 >(from, to, u32_to_u8 , avg_u32),
                (ChannelType::Float, ChannelType::  Int) => convert_pixels::<f32, i8 >(from, to, f32_to_i8 , avg_f32),
                (ChannelType::Float, ChannelType:: UInt) => convert_pixels::<f32, u8 >(from, to, f32_to_u8 , avg_f32),
                (_, _) => return Err(anyhow::anyhow!("Invalid channel type")),
            }
        (ChannelSize::_32bit, ChannelSize::_16bit) =>
            match (from.info.color_format.channel_type, to.info.color_format.channel_type) {
                (ChannelType::  Int, ChannelType::  Int) => convert_pixels::<i32, i16>(from, to, i32_to_i16, avg_i32),
                (ChannelType::  Int, ChannelType:: UInt) => convert_pixels::<i32, u16>(from, to, i32_to_u16, avg_i32),
                (ChannelType:: UInt, ChannelType::  Int) => convert_pixels::<u32, i16>(from, to, u32_to_i16, avg_u32),
                (ChannelType:: UInt, ChannelType:: UInt) => convert_pixels::<u32, u16>(from, to, u32_to_u16, avg_u32),
                (ChannelType::Float, ChannelType::  Int) => convert_pixels::<f32, i16>(from, to, f32_to_i16, avg_f32),
                (ChannelType::Float, ChannelType:: UInt) => convert_pixels::<f32, u16>(from, to, f32_to_u16, avg_f32),
                (_, _) => return Err(anyhow::anyhow!("Invalid channel type")),
            }
        (ChannelSize::_32bit, ChannelSize::_32bit) =>
            match (from.info.color_format.channel_type, to.info.color_format.channel_type) {
                (ChannelType::  Int, ChannelType::  Int) => convert_pixels::<i32, i32>(from, to, i32_to_i32, avg_i32),
                (ChannelType::  Int, ChannelType:: UInt) => convert_pixels::<i32, u32>(from, to, i32_to_u32, avg_i32),
                (ChannelType::  Int, ChannelType::Float) => convert_pixels::<i32, f32>(from, to, i32_to_f32, avg_i32),
                (ChannelType:: UInt, ChannelType::  Int) => convert_pixels::<u32, i32>(from, to, u32_to_i32, avg_u32),
                (ChannelType:: UInt, ChannelType:: UInt) => convert_pixels::<u32, u32>(from, to, u32_to_u32, avg_u32),
                (ChannelType:: UInt, ChannelType::Float) => convert_pixels::<u32, f32>(from, to, u32_to_f32, avg_u32),
                (ChannelType::Float, ChannelType::  Int) => convert_pixels::<f32, i32>(from, to, f32_to_i32, avg_f32),
                (ChannelType::Float, ChannelType:: UInt) => convert_pixels::<f32, u32>(from, to, f32_to_u32, avg_f32),
                (ChannelType::Float, ChannelType::Float) => convert_pixels::<f32, f32>(from, to, f32_to_f32, avg_f32),
            }
        (ChannelSize::_32bit, ChannelSize::_64bit) =>
            match (from.info.color_format.channel_type, to.info.color_format.channel_type) {
                (ChannelType::  Int, ChannelType::  Int) => convert_pixels::<i32, i64>(from, to, i32_to_i64, avg_i32),
                (ChannelType::  Int, ChannelType:: UInt) => convert_pixels::<i32, u64>(from, to, i32_to_u64, avg_i32),
                (ChannelType::  Int, ChannelType::Float) => convert_pixels::<i32, f64>(from, to, i32_to_f64, avg_i32),
                (ChannelType:: UInt, ChannelType::  Int) => convert_pixels::<u32, i64>(from, to, u32_to_i64, avg_u32),
                (ChannelType:: UInt, ChannelType:: UInt) => convert_pixels::<u32, u64>(from, to, u32_to_u64, avg_u32),
                (ChannelType:: UInt, ChannelType::Float) => convert_pixels::<u32, f64>(from, to, u32_to_f64, avg_u32),
                (ChannelType::Float, ChannelType::  Int) => convert_pixels::<f32, i64>(from, to, f32_to_i64, avg_f32),
                (ChannelType::Float, ChannelType:: UInt) => convert_pixels::<f32, u64>(from, to, f32_to_u64, avg_f32),
                (ChannelType::Float, ChannelType::Float) => convert_pixels::<f32, f64>(from, to, f32_to_f64, avg_f32),
            }
        (ChannelSize::_64bit, ChannelSize::_8bit) =>
            match (from.info.color_format.channel_type, to.info.color_format.channel_type) {
                (ChannelType::  Int, ChannelType::  Int) => convert_pixels::<i64, i8 >(from, to, i64_to_i8 , avg_i64),
                (ChannelType::  Int, ChannelType:: UInt) => convert_pixels::<i64, u8 >(from, to, i64_to_u8 , avg_i64),
                (ChannelType:: UInt, ChannelType::  Int) => convert_pixels::<u64, i8 >(from, to, u64_to_i8 , avg_u64),
                (ChannelType:: UInt, ChannelType:: UInt) => convert_pixels::<u64, u8 >(from, to, u64_to_u8 , avg_u64),
                (ChannelType::Float, ChannelType::  Int) => convert_pixels::<f64, i8 >(from, to, f64_to_i8 , avg_f64),
                (ChannelType::Float, ChannelType:: UInt) => convert_pixels::<f64, u8 >(from, to, f64_to_u8 , avg_f64),
                (_, _) => return Err(anyhow::anyhow!("Invalid channel type")),
            }
        (ChannelSize::_64bit, ChannelSize::_16bit) =>
            match (from.info.color_format.channel_type, to.info.color_format.channel_type) {
                (ChannelType::  Int, ChannelType::  Int) => convert_pixels::<i64, i16>(from, to, i64_to_i16, avg_i64),
                (ChannelType::  Int, ChannelType:: UInt) => convert_pixels::<i64, u16>(from, to, i64_to_u16, avg_i64),
                (ChannelType:: UInt, ChannelType::  Int) => convert_pixels::<u64, i16>(from, to, u64_to_i16, avg_u64),
                (ChannelType:: UInt, ChannelType:: UInt) => convert_pixels::<u64, u16>(from, to, u64_to_u16, avg_u64),
                (ChannelType::Float, ChannelType::  Int) => convert_pixels::<f64, i16>(from, to, f64_to_i16, avg_f64),
                (ChannelType::Float, ChannelType:: UInt) => convert_pixels::<f64, u16>(from, to, f64_to_u16, avg_f64),
                (_, _) => return Err(anyhow::anyhow!("Invalid channel type")),
            }
        (ChannelSize::_64bit, ChannelSize::_32bit) =>
            match (from.info.color_format.channel_type, to.info.color_format.channel_type) {
                (ChannelType::  Int, ChannelType::  Int) => convert_pixels::<i64, i32>(from, to, i64_to_i32, avg_i64),
                (ChannelType::  Int, ChannelType:: UInt) => convert_pixels::<i64, u32>(from, to, i64_to_u32, avg_i64),
                (ChannelType::  Int, ChannelType::Float) => convert_pixels::<i64, f32>(from, to, i64_to_f32, avg_i64),
                (ChannelType:: UInt, ChannelType::  Int) => convert_pixels::<u64, i32>(from, to, u64_to_i32, avg_u64),
                (ChannelType:: UInt, ChannelType:: UInt) => convert_pixels::<u64, u32>(from, to, u64_to_u32, avg_u64),
                (ChannelType:: UInt, ChannelType::Float) => convert_pixels::<u64, f32>(from, to, u64_to_f32, avg_u64),
                (ChannelType::Float, ChannelType::  Int) => convert_pixels::<f64, i32>(from, to, f64_to_i32, avg_f64),
                (ChannelType::Float, ChannelType:: UInt) => convert_pixels::<f64, u32>(from, to, f64_to_u32, avg_f64),
                (ChannelType::Float, ChannelType::Float) => convert_pixels::<f64, f32>(from, to, f64_to_f32, avg_f64),
            }
        (ChannelSize::_64bit, ChannelSize::_64bit) =>
            match (from.info.color_format.channel_type, to.info.color_format.channel_type) {
                (ChannelType::  Int, ChannelType::  Int) => convert_pixels::<i64, i64>(from, to, i64_to_i64, avg_i64),
                (ChannelType::  Int, ChannelType:: UInt) => convert_pixels::<i64, u64>(from, to, i64_to_u64, avg_i64),
                (ChannelType::  Int, ChannelType::Float) => convert_pixels::<i64, f64>(from, to, i64_to_f64, avg_i64),
                (ChannelType:: UInt, ChannelType::  Int) => convert_pixels::<u64, i64>(from, to, u64_to_i64, avg_u64),
                (ChannelType:: UInt, ChannelType:: UInt) => convert_pixels::<u64, u64>(from, to, u64_to_u64, avg_u64),
                (ChannelType:: UInt, ChannelType::Float) => convert_pixels::<u64, f64>(from, to, u64_to_f64, avg_u64),
                (ChannelType::Float, ChannelType::  Int) => convert_pixels::<f64, i64>(from, to, f64_to_i64, avg_f64),
                (ChannelType::Float, ChannelType:: UInt) => convert_pixels::<f64, u64>(from, to, f64_to_u64, avg_f64),
                (ChannelType::Float, ChannelType::Float) => convert_pixels::<f64, f64>(from, to, f64_to_f64, avg_f64),
            }
        // @formatter:on
    }

    Ok(())
}

type ConvertFn<From, To> = fn(&[From], &mut [To], fn(From) -> To, fn(From, From, From) -> From);


fn convert_pixels<From, To>(
    from: &Image,
    to: &mut Image,
    convert_fn: fn(From) -> To,
    avg_fn: fn(From, From, From) -> From,
)
    where From: Copy + Pod,
          To: Copy + Pod + Bounded,
{
    assert_eq!(from.info.width, to.info.width);
    assert_eq!(from.info.height, to.info.height);
    assert_eq!(from.info.color_format.channel_size.byte_count(), size_of::<From>() as u32);
    assert_eq!(to.info.color_format.channel_size.byte_count(), size_of::<To>() as u32);

    if from.info.color_format.channel_count == to.info.color_format.channel_count
        && from.info.color_format.channel_size == to.info.color_format.channel_size {
        return;
    }

    let to_pixel_size = to.info.color_format.byte_count();
    let from_pixel_size = from.info.color_format.byte_count();


    let convert_pixel: ConvertFn<From, To> =
        match (to.info.color_format.channel_count, from.info.color_format.channel_count) {
            (ChannelCount::Gray, ChannelCount::Gray) =>
                |from_pixel, to_pixel, convert_fn, _| {
                    to_pixel[0] = convert_fn(from_pixel[0]);
                },
            (ChannelCount::Gray, ChannelCount::GrayAlpha) =>
                |from_pixel, to_pixel, convert_fn, _| {
                    to_pixel[0] = convert_fn(from_pixel[0]);
                },
            (ChannelCount::Gray, ChannelCount::Rgb) =>
                |from_pixel, to_pixel, convert_fn, avg_fn| {
                    to_pixel[0] = convert_fn(avg_fn(from_pixel[0], from_pixel[1], from_pixel[2]));
                },
            (ChannelCount::Gray, ChannelCount::Rgba) =>
                |from_pixel, to_pixel, convert_fn, avg_fn| {
                    to_pixel[0] = convert_fn(avg_fn(from_pixel[0], from_pixel[1], from_pixel[2]));
                },

            (ChannelCount::GrayAlpha, ChannelCount::Gray) =>
                |from_pixel, to_pixel, convert_fn, _| {
                    to_pixel[0] = convert_fn(from_pixel[0]);
                    to_pixel[1] = To::max_value();
                },
            (ChannelCount::GrayAlpha, ChannelCount::GrayAlpha) =>
                |from_pixel, to_pixel, convert_fn, _| {
                    to_pixel[0] = convert_fn(from_pixel[0]);
                    to_pixel[1] = convert_fn(from_pixel[1]);
                },
            (ChannelCount::GrayAlpha, ChannelCount::Rgb) =>
                |from_pixel, to_pixel, convert_fn, avg_fn| {
                    to_pixel[0] = convert_fn(avg_fn(from_pixel[0], from_pixel[1], from_pixel[2]));
                    to_pixel[1] = To::max_value();
                },
            (ChannelCount::GrayAlpha, ChannelCount::Rgba) =>
                |from_pixel, to_pixel, convert_fn, avg_fn| {
                    to_pixel[0] = convert_fn(avg_fn(from_pixel[0], from_pixel[1], from_pixel[2]));
                    to_pixel[1] = convert_fn(from_pixel[3]);
                },

            (ChannelCount::Rgb, ChannelCount::Gray) =>
                |from_pixel, to_pixel, convert_fn, _| {
                    to_pixel[0] = convert_fn(from_pixel[0]);
                    to_pixel[1] = to_pixel[0];
                    to_pixel[2] = to_pixel[0];
                },
            (ChannelCount::Rgb, ChannelCount::GrayAlpha) =>
                |from_pixel, to_pixel, convert_fn, _| {
                    to_pixel[0] = convert_fn(from_pixel[0]);
                    to_pixel[1] = to_pixel[0];
                    to_pixel[2] = to_pixel[0];
                },
            (ChannelCount::Rgb, ChannelCount::Rgb) =>
                |from_pixel, to_pixel, convert_fn, _| {
                    to_pixel[0] = convert_fn(from_pixel[0]);
                    to_pixel[1] = convert_fn(from_pixel[1]);
                    to_pixel[2] = convert_fn(from_pixel[2]);
                },
            (ChannelCount::Rgb, ChannelCount::Rgba) =>
                |from_pixel, to_pixel, convert_fn, _| {
                    to_pixel[0] = convert_fn(from_pixel[0]);
                    to_pixel[1] = convert_fn(from_pixel[1]);
                    to_pixel[2] = convert_fn(from_pixel[2]);
                },

            (ChannelCount::Rgba, ChannelCount::Gray) =>
                |from_pixel, to_pixel, convert_fn, _| {
                    to_pixel[0] = convert_fn(from_pixel[0]);
                    to_pixel[1] = to_pixel[0];
                    to_pixel[2] = to_pixel[0];
                    to_pixel[3] = To::max_value();
                },
            (ChannelCount::Rgba, ChannelCount::GrayAlpha) =>
                |from_pixel, to_pixel, convert_fn, _| {
                    to_pixel[0] = convert_fn(from_pixel[0]);
                    to_pixel[1] = to_pixel[0];
                    to_pixel[2] = to_pixel[0];
                    to_pixel[3] = convert_fn(from_pixel[1]);
                },
            (ChannelCount::Rgba, ChannelCount::Rgb) =>
                |from_pixel, to_pixel, convert_fn, _| {
                    to_pixel[0] = convert_fn(from_pixel[0]);
                    to_pixel[1] = convert_fn(from_pixel[1]);
                    to_pixel[2] = convert_fn(from_pixel[2]);
                    to_pixel[3] = To::max_value();
                },
            (ChannelCount::Rgba, ChannelCount::Rgba) =>
                |from_pixel, to_pixel, convert_fn, _| {
                    to_pixel[0] = convert_fn(from_pixel[0]);
                    to_pixel[1] = convert_fn(from_pixel[1]);
                    to_pixel[2] = convert_fn(from_pixel[2]);
                    to_pixel[3] = convert_fn(from_pixel[3]);
                },
        };


    for i in 0..from.info.height {
        for j in 0..from.info.width {
            let from_offset = i * from.info.stride + j * from_pixel_size;
            let from_pixel: &[From] = bytemuck::cast_slice(
                &from.bytes[from_offset as usize..(from_offset + from_pixel_size) as usize]
            );

            let to_offset = i * to.info.stride + j * to_pixel_size;
            let to_pixel: &mut [To] = bytemuck::cast_slice_mut(
                &mut to.bytes[to_offset as usize..(to_offset + to_pixel_size) as usize]
            );

            convert_pixel(from_pixel, to_pixel, convert_fn, avg_fn);
        }
    }
}

// @formatter:off
#[inline] pub(crate) fn  i8_to_i8 (value: i8) ->  i8 {
    value
}
#[inline] pub(crate) fn  i8_to_i16(value: i8) -> i16 {
    (value as i16) << 8 | value as i16
}
#[inline] pub(crate) fn  i8_to_i32(value: i8) -> i32 {
    (value as i32) << 24 | (value as i32) << 16 | (value as i32) << 8 | value as i32
}
#[inline] pub(crate) fn  i8_to_i64(value: i8) -> i64 {
    (value as i64) << 56 | (value as i64) << 48 | (value as i64) << 40 | (value as i64) << 32
        | (value as i64) << 24 | (value as i64) << 16 | (value as i64) << 8 | value as i64
}

#[inline] pub(crate) fn i16_to_i8 (value: i16) ->  i8 {
    (value >> 8) as i8
}
#[inline] pub(crate) fn i16_to_i16(value: i16) -> i16 {
    value
}
#[inline] pub(crate) fn i16_to_i32(value: i16) -> i32 {
    (value as i32) << 16 | value as i32
}
#[inline] pub(crate) fn i16_to_i64(value: i16) -> i64 {
    (value as i64) << 48 | value as i64
}

#[inline] pub(crate) fn i32_to_i8 (value: i32) ->  i8 {
    (value >> 24) as i8
}
#[inline] pub(crate) fn i32_to_i16(value: i32) -> i16 {
    (value >> 16) as i16
}
#[inline] pub(crate) fn i32_to_i32(value: i32) -> i32 {
    value
}
#[inline] pub(crate) fn i32_to_i64(value: i32) -> i64 {
    (value as i64) << 32 | value as i64
}

#[inline] pub(crate) fn i64_to_i8 (value: i64) ->  i8 {
    (value >> 56) as i8
}
#[inline] pub(crate) fn i64_to_i16(value: i64) -> i16 {
    (value >> 48) as i16 
} 
#[inline] pub(crate) fn i64_to_i32(value: i64) -> i32 {
    (value >> 32) as i32 
} 
#[inline] pub(crate) fn i64_to_i64(value: i64) -> i64 {
    value
}

#[inline] pub(crate) fn  u8_to_u8 (value: u8) ->  u8 {
    value
}
#[inline] pub(crate) fn  u8_to_u16(value: u8) -> u16 {
    (value as u16) << 8 | value as u16
}
#[inline] pub(crate) fn  u8_to_u32(value: u8) -> u32 {
    (value as u32) << 24 | (value as u32) << 16 | (value as u32) << 8 | value as u32
}
#[inline] pub(crate) fn  u8_to_u64(value: u8) -> u64 {
    (value as u64) << 56 | (value as u64) << 48 | (value as u64) << 40 | (value as u64) << 32
        | (value as u64) << 24 | (value as u64) << 16 | (value as u64) << 8 | value as u64
}

#[inline] pub(crate) fn u16_to_u8 (value: u16) ->  u8 {
    (value >> 8) as u8
}
#[inline] pub(crate) fn u16_to_u16(value: u16) -> u16 {
    value
}
#[inline] pub(crate) fn u16_to_u32(value: u16) -> u32 {
    (value as u32) << 16 | value as u32
}
#[inline] pub(crate) fn u16_to_u64(value: u16) -> u64 {
    (value as u64) << 48 | (value as u64) << 32 | (value as u64) << 16 | value as u64
}

#[inline] pub(crate) fn u32_to_u8 (value: u32) ->  u8 {
    (value >> 24) as u8
}
#[inline] pub(crate) fn u32_to_u16(value: u32) -> u16 {
    (value >> 16) as u16
}
#[inline] pub(crate) fn u32_to_u32(value: u32) -> u32 {
    value
}
#[inline] pub(crate) fn u32_to_u64(value: u32) -> u64 {
    (value as u64) << 32 | value as u64
}

#[inline] pub(crate) fn u64_to_u8 (value: u64) ->  u8 {
    (value >> 56) as u8
}
#[inline] pub(crate) fn u64_to_u16(value: u64) -> u16 {
    (value >> 48) as u16
}
#[inline] pub(crate) fn u64_to_u32(value: u64) -> u32 {
    (value >> 32) as u32
}
#[inline] pub(crate) fn u64_to_u64(value: u64) -> u64 {
    value
}

#[inline] pub(crate) fn  u8_to_i8 (value: u8) ->  i8 {
    (value / 2) as i8
}
#[inline] pub(crate) fn  u8_to_i16(value: u8) -> i16 {
    i8_to_i16(u8_to_i8(value))
}
#[inline] pub(crate) fn  u8_to_i32(value: u8) -> i32 {
    i8_to_i32(u8_to_i8(value))
}
#[inline] pub(crate) fn  u8_to_i64(value: u8) -> i64 {
    i8_to_i64(u8_to_i8(value))
}

#[inline] pub(crate) fn u16_to_i8 (value: u16) ->  i8 {
    i16_to_i8(u16_to_i16(value))
}
#[inline] pub(crate) fn u16_to_i16(value: u16) -> i16 {
    (value / 2) as i16
}
#[inline] pub(crate) fn u16_to_i32(value: u16) -> i32 {
    i16_to_i32(u16_to_i16(value))
}
#[inline] pub(crate) fn u16_to_i64(value: u16) -> i64 {
    i16_to_i64(u16_to_i16(value))
}

#[inline] pub(crate) fn u32_to_i8 (value: u32) ->  i8 {
    i32_to_i8(u32_to_i32(value))
}
#[inline] pub(crate) fn u32_to_i16(value: u32) -> i16 {
    i32_to_i16(u32_to_i32(value))
}
#[inline] pub(crate) fn u32_to_i32(value: u32) -> i32 {
    (value / 2) as i32
}
#[inline] pub(crate) fn u32_to_i64(value: u32) -> i64 {
    i32_to_i64(u32_to_i32(value))
}

#[inline] pub(crate) fn u64_to_i8 (value: u64) ->  i8 {
    i64_to_i8(u64_to_i64(value))
}
#[inline] pub(crate) fn u64_to_i16(value: u64) -> i16 {
    i64_to_i16(u64_to_i64(value))
}
#[inline] pub(crate) fn u64_to_i32(value: u64) -> i32 {
    i64_to_i32(u64_to_i64(value))
}
#[inline] pub(crate) fn u64_to_i64(value: u64) -> i64 {
    (value / 2) as i64
}

#[inline] pub(crate) fn  i8_to_u8 (value: i8) ->  u8 {
    value.max(0) as u8 * 2
}
#[inline] pub(crate) fn  i8_to_u16(value: i8) -> u16 {
    u8_to_u16(i8_to_u8(value))
}
#[inline] pub(crate) fn  i8_to_u32(value: i8) -> u32 {
    u8_to_u32(i8_to_u8(value))
}
#[inline] pub(crate) fn  i8_to_u64(value: i8) -> u64 {
    u8_to_u64(i8_to_u8(value))
}

#[inline] pub(crate) fn i16_to_u8 (value: i16) ->  u8 {
    u16_to_u8(i16_to_u16(value))
}
#[inline] pub(crate) fn i16_to_u16(value: i16) -> u16 {
    value.max(0) as u16 * 2
}
#[inline] pub(crate) fn i16_to_u32(value: i16) -> u32 {
    u16_to_u32(i16_to_u16(value))
}
#[inline] pub(crate) fn i16_to_u64(value: i16) -> u64 {
    u16_to_u64(i16_to_u16(value))
}

#[inline] pub(crate) fn i32_to_u8 (value: i32) ->  u8 {
    u32_to_u8(i32_to_u32(value))
}
#[inline] pub(crate) fn i32_to_u16(value: i32) -> u16 {
    u32_to_u16(i32_to_u32(value))
}
#[inline] pub(crate) fn i32_to_u32(value: i32) -> u32 {
    value.max(0) as u32 * 2
}
#[inline] pub(crate) fn i32_to_u64(value: i32) -> u64 {
    u32_to_u64(i32_to_u32(value))
}

#[inline] pub(crate) fn i64_to_u8 (value: i64) ->  u8 {
    u64_to_u8(i64_to_u64(value))
}
#[inline] pub(crate) fn i64_to_u16(value: i64) -> u16 {
    u64_to_u16(i64_to_u64(value))
}
#[inline] pub(crate) fn i64_to_u32(value: i64) -> u32 {
    u64_to_u32(i64_to_u64(value))
}
#[inline] pub(crate) fn i64_to_u64(value: i64) -> u64 {
    value.max(0) as u64 * 2
}

#[inline] pub(crate) fn  u8_to_f32(value:  u8) -> f32 { value as f32 /  u8::MAX as f32 }
#[inline] pub(crate) fn u16_to_f32(value: u16) -> f32 { value as f32 / u16::MAX as f32 }
#[inline] pub(crate) fn u32_to_f32(value: u32) -> f32 { value as f32 / u32::MAX as f32 }
#[inline] pub(crate) fn u64_to_f32(value: u64) -> f32 { value as f32 / u64::MAX as f32 }

#[inline] pub(crate) fn  u8_to_f64(value:  u8) -> f64 { value as f64 /  u8::MAX as f64 }
#[inline] pub(crate) fn u16_to_f64(value: u16) -> f64 { value as f64 / u16::MAX as f64 }
#[inline] pub(crate) fn u32_to_f64(value: u32) -> f64 { value as f64 / u32::MAX as f64 }
#[inline] pub(crate) fn u64_to_f64(value: u64) -> f64 { value as f64 / u64::MAX as f64 }

#[inline] pub(crate) fn  i8_to_f32(value:  i8) -> f32 { value as f32 /  i8::MAX as f32 }
#[inline] pub(crate) fn i16_to_f32(value: i16) -> f32 { value as f32 / i16::MAX as f32 }
#[inline] pub(crate) fn i32_to_f32(value: i32) -> f32 { value as f32 / i32::MAX as f32 }
#[inline] pub(crate) fn i64_to_f32(value: i64) -> f32 { value as f32 / i64::MAX as f32 }

#[inline] pub(crate) fn  i8_to_f64(value:  i8) -> f64 { value as f64 /  i8::MAX as f64 }
#[inline] pub(crate) fn i16_to_f64(value: i16) -> f64 { value as f64 / i16::MAX as f64 }
#[inline] pub(crate) fn i32_to_f64(value: i32) -> f64 { value as f64 / i32::MAX as f64 }
#[inline] pub(crate) fn i64_to_f64(value: i64) -> f64 { value as f64 / i64::MAX as f64 }

#[inline] pub(crate) fn f32_to_u8 (value: f32) ->  u8 { (value as f64 *  u8::MAX as f64) as  u8 }
#[inline] pub(crate) fn f32_to_u16(value: f32) -> u16 { (value as f64 * u16::MAX as f64) as u16 }
#[inline] pub(crate) fn f32_to_u32(value: f32) -> u32 { (value as f64 * u32::MAX as f64) as u32 }
#[inline] pub(crate) fn f32_to_u64(value: f32) -> u64 { (value as f64 * u64::MAX as f64) as u64 }

#[inline] pub(crate) fn f64_to_u8 (value: f64) ->  u8 { (value *  u8::MAX as f64) as  u8 }
#[inline] pub(crate) fn f64_to_u16(value: f64) -> u16 { (value * u16::MAX as f64) as u16 }
#[inline] pub(crate) fn f64_to_u32(value: f64) -> u32 { (value * u32::MAX as f64) as u32 }
#[inline] pub(crate) fn f64_to_u64(value: f64) -> u64 { (value * u64::MAX as f64) as u64 }

#[inline] pub(crate) fn f32_to_i8 (value: f32) ->  i8 { (value as f64 *  i8::MAX as f64) as  i8 }
#[inline] pub(crate) fn f32_to_i16(value: f32) -> i16 { (value as f64 * i16::MAX as f64) as i16 }
#[inline] pub(crate) fn f32_to_i32(value: f32) -> i32 { (value as f64 * i32::MAX as f64) as i32 }
#[inline] pub(crate) fn f32_to_i64(value: f32) -> i64 { (value as f64 * i64::MAX as f64) as i64 }

#[inline] pub(crate) fn f64_to_i8 (value: f64) ->  i8 { (value *  i8::MAX as f64) as  i8 }
#[inline] pub(crate) fn f64_to_i16(value: f64) -> i16 { (value * i16::MAX as f64) as i16 }
#[inline] pub(crate) fn f64_to_i32(value: f64) -> i32 { (value * i32::MAX as f64) as i32 }
#[inline] pub(crate) fn f64_to_i64(value: f64) -> i64 { (value * i64::MAX as f64) as i64 }

#[inline] pub(crate) fn f32_to_f64(value: f32) -> f64 { value as f64 }
#[inline] pub(crate) fn f32_to_f32(value: f32) -> f32 { value }
#[inline] pub(crate) fn f64_to_f32(value: f64) -> f32 { value as f32 }
#[inline] pub(crate) fn f64_to_f64(value: f64) -> f64 { value }


#[inline] pub(crate) fn avg_u8 (v0:  u8, v1:  u8, v2:  u8) ->  u8 { ((v0 as  u16 + v1 as  u16 + v2 as  u16) / 3) as  u8 }
#[inline] pub(crate) fn avg_u16(v0: u16, v1: u16, v2: u16) -> u16 { ((v0 as  u32 + v1 as  u32 + v2 as  u32) / 3) as u16 }
#[inline] pub(crate) fn avg_u32(v0: u32, v1: u32, v2: u32) -> u32 { ((v0 as  u64 + v1 as  u64 + v2 as  u64) / 3) as u32 }
#[inline] pub(crate) fn avg_u64(v0: u64, v1: u64, v2: u64) -> u64 { ((v0 as u128 + v1 as u128 + v2 as u128) / 3) as u64 }

#[inline] pub(crate) fn avg_i8 (v0:  i8, v1:  i8, v2:  i8) ->  i8 { ((v0 as  i16 + v1 as  i16 + v2 as  i16) / 3) as  i8 }
#[inline] pub(crate) fn avg_i16(v0: i16, v1: i16, v2: i16) -> i16 { ((v0 as  i32 + v1 as  i32 + v2 as  i32) / 3) as i16 }
#[inline] pub(crate) fn avg_i32(v0: i32, v1: i32, v2: i32) -> i32 { ((v0 as  i64 + v1 as  i64 + v2 as  i64) / 3) as i32 }
#[inline] pub(crate) fn avg_i64(v0: i64, v1: i64, v2: i64) -> i64 { ((v0 as i128 + v1 as i128 + v2 as i128) / 3) as i64 }

#[inline] pub(crate) fn avg_f32(v0: f32, v1: f32, v2: f32) -> f32 { ((v0 as f64 + v1 as f64 + v2 as f64) / 3.0) as f32 }
#[inline] pub(crate) fn avg_f64(v0: f64, v1: f64, v2: f64) -> f64 { (v0 + v1 + v2) / 3.0 }

// @formatter:on
