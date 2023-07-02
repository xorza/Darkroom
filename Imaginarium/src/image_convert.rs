use std::mem::size_of;

use bytemuck::Pod;
use num_traits::{Bounded, NumCast, ToPrimitive};

use crate::image::{ChannelCount, Image};

#[inline]
pub(crate) fn u8_to_u8(value: u8) -> u8 {
    value
}
#[inline]
pub(crate) fn u8_to_u16(value: u8) -> u16 {
    (value as u16) << 8 | value as u16
}
#[inline]
pub(crate) fn u8_to_u32(value: u8) -> u32 {
    (value as u32) << 24 | (value as u32) << 16 | (value as u32) << 8 | value as u32
}
#[inline]
pub(crate) fn u8_to_u64(value: u8) -> u64 {
    (value as u64) << 56 | (value as u64) << 48 | (value as u64) << 40 | (value as u64) << 32
        | (value as u64) << 24 | (value as u64) << 16 | (value as u64) << 8 | value as u64
}
#[inline]
pub(crate) fn u16_to_u8(value: u16) -> u8 {
    (value >> 8) as u8
}
#[inline]
pub(crate) fn u16_to_u16(value: u16) -> u16 {
    value
}
#[inline]
pub(crate) fn u16_to_u32(value: u16) -> u32 {
    (value as u32) << 16 | value as u32
}
#[inline]
pub(crate) fn u16_to_u64(value: u16) -> u64 {
    (value as u64) << 48 | (value as u64) << 32 | (value as u64) << 16 | value as u64
}
#[inline]
pub(crate) fn u32_to_u8(value: u32) -> u8 {
    (value >> 24) as u8
}
#[inline]
pub(crate) fn u32_to_u16(value: u32) -> u16 {
    (value >> 16) as u16
}
#[inline]
pub(crate) fn u32_to_u32(value: u32) -> u32 {
    value
}
#[inline]
pub(crate) fn u32_to_u64(value: u32) -> u64 {
    (value as u64) << 32 | value as u64
}
#[inline]
pub(crate) fn u64_to_u8(value: u64) -> u8 {
    (value >> 56) as u8
}
#[inline]
pub(crate) fn u64_to_u16(value: u64) -> u16 {
    (value >> 48) as u16
}
#[inline]
pub(crate) fn u64_to_u32(value: u64) -> u32 {
    (value >> 32) as u32
}
#[inline]
pub(crate) fn u64_to_u64(value: u64) -> u64 {
    value
}
#[inline]
pub(crate) fn avg_u8(v0: u8, v1: u8, v2: u8) -> u8 {
    ((v0 as u32 + v1 as u32 + v2 as u32) / 3) as u8
}
#[inline]
pub(crate) fn avg_u16(v0: u16, v1: u16, v2: u16) -> u16 {
    ((v0 as u32 + v1 as u32 + v2 as u32) / 3) as u16
}
#[inline]
pub(crate) fn avg_u32(v0: u32, v1: u32, v2: u32) -> u32 {
    ((v0 as u64 + v1 as u64 + v2 as u64) / 3) as u32
}
#[inline]
pub(crate) fn avg_u64(v0: u64, v1: u64, v2: u64) -> u64 {
    ((v0 as u128 + v1 as u128 + v2 as u128) / 3) as u64
}

pub(crate) fn convert<From, To>(
    from: &Image,
    to: &mut Image,
    convert_fn: fn(From) -> To,
    avg_fn: fn(From, From, From) -> From,
)
    where From: Copy + Pod,
          To: Copy + Pod + Bounded,
{
    assert_eq!(from.width, to.width);
    assert_eq!(from.height, to.height);
    assert_eq!(from.channel_size.byte_count(), size_of::<From>() as u32);
    assert_eq!(to.channel_size.byte_count(), size_of::<To>() as u32);

    if from.channel_count == to.channel_count
        && from.channel_size == to.channel_size {
        return;
    }

    let to_pixel_size = to.channel_count.byte_count(to.channel_size) as usize;
    let from_pixel_size = from.channel_count.byte_count(from.channel_size) as usize;

    let convert_pixel: fn(&[From], &mut [To], fn(From) -> To, fn(From, From, From) -> From) =
        match (to.channel_count, from.channel_count) {
            (ChannelCount::Gray, ChannelCount::GrayAlpha) =>
                |from_pixel, to_pixel, convert_fn, _avg_fn| {
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
                |from_pixel, to_pixel, convert_fn, _avg_fn| {
                    to_pixel[0] = convert_fn(from_pixel[0]);
                    to_pixel[1] = To::max_value();
                },
            (ChannelCount::GrayAlpha, ChannelCount::Rgb) =>
                |from_pixel, to_pixel, convert_fn, avg_fn| {
                    to_pixel[0] = convert_fn(avg_fn(from_pixel[0], from_pixel[1], from_pixel[2]));
                    to_pixel[1] = To::max_value();
                },
            (ChannelCount::GrayAlpha, ChannelCount::Rgba) =>
                |from_pixel, to_pixel, convert_fn, avg_fn| {
                    to_pixel[0] = convert_fn(avg_fn(from_pixel[0], from_pixel[1], from_pixel[2]));
                    to_pixel[1] = convert_fn(from_pixel[1]);
                },

            (ChannelCount::Rgb, ChannelCount::Gray) =>
                |from_pixel, to_pixel, convert_fn, _avg_fn| {
                    to_pixel[0] = convert_fn(from_pixel[0]);
                    to_pixel[1] = to_pixel[0];
                    to_pixel[2] = to_pixel[0];
                },
            (ChannelCount::Rgb, ChannelCount::GrayAlpha) =>
                |from_pixel, to_pixel, convert_fn, _avg_fn| {
                    to_pixel[0] = convert_fn(from_pixel[0]);
                    to_pixel[1] = to_pixel[0];
                    to_pixel[2] = to_pixel[0];
                },
            (ChannelCount::Rgb, ChannelCount::Rgba) =>
                |from_pixel, to_pixel, convert_fn, _avg_fn| {
                    to_pixel[0] = convert_fn(from_pixel[0]);
                    to_pixel[1] = convert_fn(from_pixel[1]);
                    to_pixel[2] = convert_fn(from_pixel[2]);
                },

            (ChannelCount::Rgba, ChannelCount::Gray) =>
                |from_pixel, to_pixel, convert_fn, _avg_fn| {
                    to_pixel[0] = convert_fn(from_pixel[0]);
                    to_pixel[1] = to_pixel[0];
                    to_pixel[2] = to_pixel[0];
                    to_pixel[3] = To::max_value();
                },
            (ChannelCount::Rgba, ChannelCount::GrayAlpha) =>
                |from_pixel, to_pixel, convert_fn, _avg_fn| {
                    to_pixel[0] = convert_fn(from_pixel[0]);
                    to_pixel[1] = to_pixel[0];
                    to_pixel[2] = to_pixel[0];
                    to_pixel[3] = convert_fn(from_pixel[1]);
                },
            (ChannelCount::Rgba, ChannelCount::Rgb) =>
                |from_pixel, to_pixel, convert_fn, _avg_fn| {
                    to_pixel[0] = convert_fn(from_pixel[0]);
                    to_pixel[1] = convert_fn(from_pixel[1]);
                    to_pixel[2] = convert_fn(from_pixel[2]);
                    to_pixel[3] = To::max_value();
                },

            _ => panic!("Unsupported channel count conversion: {:?} -> {:?}", from.channel_count, to.channel_count),
        };


    for i in 0..from.height as usize {
        for j in 0..from.width as usize {
            let from_offset = i * from.stride as usize + j * from_pixel_size;
            let from_pixel: &[From] = bytemuck::cast_slice(
                &from.bytes[from_offset..from_offset + from_pixel_size]
            );

            let to_offset = i * to.stride as usize + j * to_pixel_size;
            let to_pixel: &mut [To] = bytemuck::cast_slice_mut(
                &mut to.bytes[to_offset..to_offset + to_pixel_size]
            );

            convert_pixel(from_pixel, to_pixel, convert_fn, avg_fn);
        }
    }
}


// match (to.channel_count, from.channel_count) {
// (ChannelCount::Gray, ChannelCount::GrayAlpha) => {
// to_pixel[0] = convert_fn(from_pixel[0]);
// }
// (ChannelCount::Gray, ChannelCount::Rgb) => {
// to_pixel[0] = convert_fn(avg_fn(from_pixel[0], from_pixel[1], from_pixel[2]));
// }
// (ChannelCount::Gray, ChannelCount::Rgba) => {
// to_pixel[0] = convert_fn(avg_fn(from_pixel[0], from_pixel[1], from_pixel[2]));
// }
//
// (ChannelCount::GrayAlpha, ChannelCount::Gray) => {
// to_pixel[0] = convert_fn(from_pixel[0]);
// to_pixel[1] = To::max_value();
// }
// (ChannelCount::GrayAlpha, ChannelCount::Rgb) => {
// to_pixel[0] = convert_fn(avg_fn(from_pixel[0], from_pixel[1], from_pixel[2]));
// to_pixel[1] = To::max_value();
// }
// (ChannelCount::GrayAlpha, ChannelCount::Rgba) => {
// to_pixel[0] = convert_fn(avg_fn(from_pixel[0], from_pixel[1], from_pixel[2]));
// to_pixel[1] = convert_fn(from_pixel[1]);
// }
//
// (ChannelCount::Rgb, ChannelCount::Gray) => {
// to_pixel[0] = convert_fn(from_pixel[0]);
// to_pixel[1] = to_pixel[0];
// to_pixel[2] = to_pixel[0];
// }
// (ChannelCount::Rgb, ChannelCount::GrayAlpha) => {
// to_pixel[0] = convert_fn(from_pixel[0]);
// to_pixel[1] = to_pixel[0];
// to_pixel[2] = to_pixel[0];
// }
// (ChannelCount::Rgb, ChannelCount::Rgba) => {
// to_pixel[0] = convert_fn(from_pixel[0]);
// to_pixel[1] = convert_fn(from_pixel[1]);
// to_pixel[2] = convert_fn(from_pixel[2]);
// }
//
// (ChannelCount::Rgba, ChannelCount::Gray) => {
// to_pixel[0] = convert_fn(from_pixel[0]);
// to_pixel[1] = to_pixel[0];
// to_pixel[2] = to_pixel[0];
// to_pixel[3] = To::max_value();
// }
// (ChannelCount::Rgba, ChannelCount::GrayAlpha) => {
// to_pixel[0] = convert_fn(from_pixel[0]);
// to_pixel[1] = to_pixel[0];
// to_pixel[2] = to_pixel[0];
// to_pixel[3] = convert_fn(from_pixel[1]);
// }
// (ChannelCount::Rgba, ChannelCount::Rgb) => {
// to_pixel[0] = convert_fn(from_pixel[0]);
// to_pixel[1] = convert_fn(from_pixel[1]);
// to_pixel[2] = convert_fn(from_pixel[2]);
// to_pixel[3] = To::max_value();
// }
//
// _ => panic!("Unsupported channel count conversion: {:?} -> {:?}", from.channel_count, to.channel_count),
// }
