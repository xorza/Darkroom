use std::mem::size_of;

use bytemuck::Pod;

use crate::common::color_format::*;

// =============================================================================
// Internal trait for channel conversion
// =============================================================================

pub(crate) trait ChannelConvert<To>: Copy {
    fn convert(self) -> To;
}

/// Trait for computing luminance from RGB channels.
/// Uses Rec. 709 (sRGB) weights: 0.2126*R + 0.7152*G + 0.0722*B
pub(crate) trait RgbToLuminance: Copy {
    fn luminance(r: Self, g: Self, b: Self) -> Self;
}

/// Trait for getting the opaque alpha value for a channel type.
/// For integers this is max value (255, 65535), for floats it's 1.0.
pub trait OpaqueAlpha: Copy {
    fn opaque_alpha() -> Self;
}

// =============================================================================
// Macro to implement ChannelConvert for all type combinations
// =============================================================================

// Identity conversions
macro_rules! impl_convert_identity {
    ($($t:ty),+) => {
        $(
            impl ChannelConvert<$t> for $t {
                #[inline]
                fn convert(self) -> $t { self }
            }
        )+
    };
}

impl_convert_identity!(u8, u16, f32);

// Upscale: replicate bits to fill larger type
macro_rules! impl_convert_upscale_unsigned {
    ($from:ty, $to:ty) => {
        impl ChannelConvert<$to> for $from {
            #[inline]
            fn convert(self) -> $to {
                let shift = (size_of::<$to>() - size_of::<$from>()) * 8;
                (self as $to) << shift | (self as $to)
            }
        }
    };
}

// Downscale: take high bits
macro_rules! impl_convert_downscale {
    ($from:ty, $to:ty) => {
        impl ChannelConvert<$to> for $from {
            #[inline]
            fn convert(self) -> $to {
                let shift = (size_of::<$from>() - size_of::<$to>()) * 8;
                (self >> shift) as $to
            }
        }
    };
}

// Integer to float
macro_rules! impl_convert_int_to_float {
    ($int:ty, $float:ty) => {
        impl ChannelConvert<$float> for $int {
            #[inline]
            fn convert(self) -> $float {
                self as $float / <$int>::MAX as $float
            }
        }
    };
}

// Float to integer
macro_rules! impl_convert_float_to_int {
    ($float:ty, $int:ty) => {
        impl ChannelConvert<$int> for $float {
            #[inline]
            fn convert(self) -> $int {
                (self as f64 * <$int>::MAX as f64) as $int
            }
        }
    };
}

// =============================================================================
// Implement all conversions
// =============================================================================

// Unsigned upscale
impl_convert_upscale_unsigned!(u8, u16);

// Unsigned downscale
impl_convert_downscale!(u16, u8);

// Integer to float
impl_convert_int_to_float!(u8, f32);
impl_convert_int_to_float!(u16, f32);

// Float to integer
impl_convert_float_to_int!(f32, u8);
impl_convert_float_to_int!(f32, u16);

// =============================================================================
// Implement RgbToLuminance
// =============================================================================

use super::{LUMA_B, LUMA_G, LUMA_R};

impl RgbToLuminance for u8 {
    #[inline]
    fn luminance(r: Self, g: Self, b: Self) -> Self {
        ((r as u32 * LUMA_R + g as u32 * LUMA_G + b as u32 * LUMA_B) >> 16) as u8
    }
}

impl RgbToLuminance for u16 {
    #[inline]
    fn luminance(r: Self, g: Self, b: Self) -> Self {
        ((r as u64 * LUMA_R as u64 + g as u64 * LUMA_G as u64 + b as u64 * LUMA_B as u64) >> 16)
            as u16
    }
}

impl RgbToLuminance for f32 {
    #[inline]
    fn luminance(r: Self, g: Self, b: Self) -> Self {
        0.2126 * r + 0.7152 * g + 0.0722 * b
    }
}

// =============================================================================
// Implement OpaqueAlpha
// =============================================================================

impl OpaqueAlpha for u8 {
    #[inline]
    fn opaque_alpha() -> Self {
        255
    }
}

impl OpaqueAlpha for u16 {
    #[inline]
    fn opaque_alpha() -> Self {
        65535
    }
}

impl OpaqueAlpha for f32 {
    #[inline]
    fn opaque_alpha() -> Self {
        1.0
    }
}

// =============================================================================
// Row conversion functions (scalar implementation)
// =============================================================================

/// Convert a single row of pixels using scalar code.
/// This is the fallback when SIMD is not available.
#[inline]
pub(crate) fn convert_row_scalar<From, To>(
    from_row: &[u8],
    to_row: &mut [u8],
    width: usize,
    from_channels: usize,
    to_channels: usize,
) where
    From: Pod + ChannelConvert<To> + RgbToLuminance,
    To: Pod + OpaqueAlpha,
{
    let from_row_bytes = width * from_channels * size_of::<From>();
    let to_row_bytes = width * to_channels * size_of::<To>();

    let from_row: &[From] = bytemuck::cast_slice(&from_row[..from_row_bytes]);
    let to_row: &mut [To] = bytemuck::cast_slice_mut(&mut to_row[..to_row_bytes]);

    for x in 0..width {
        let src = &from_row[x * from_channels..];
        let dst = &mut to_row[x * to_channels..];

        match (to_channels, from_channels) {
            (1, 1) => dst[0] = src[0].convert(),
            (1, 2) => dst[0] = src[0].convert(),
            (1, 3) => dst[0] = From::luminance(src[0], src[1], src[2]).convert(),
            (1, 4) => dst[0] = From::luminance(src[0], src[1], src[2]).convert(),

            (2, 1) => {
                dst[0] = src[0].convert();
                dst[1] = To::opaque_alpha();
            }
            (2, 2) => {
                dst[0] = src[0].convert();
                dst[1] = src[1].convert();
            }
            (2, 3) => {
                dst[0] = From::luminance(src[0], src[1], src[2]).convert();
                dst[1] = To::opaque_alpha();
            }
            (2, 4) => {
                dst[0] = From::luminance(src[0], src[1], src[2]).convert();
                dst[1] = src[3].convert();
            }

            (3, 1) => {
                let v = src[0].convert();
                dst[0] = v;
                dst[1] = v;
                dst[2] = v;
            }
            (3, 2) => {
                let v = src[0].convert();
                dst[0] = v;
                dst[1] = v;
                dst[2] = v;
            }
            (3, 3) => {
                dst[0] = src[0].convert();
                dst[1] = src[1].convert();
                dst[2] = src[2].convert();
            }
            (3, 4) => {
                dst[0] = src[0].convert();
                dst[1] = src[1].convert();
                dst[2] = src[2].convert();
            }

            (4, 1) => {
                let v = src[0].convert();
                dst[0] = v;
                dst[1] = v;
                dst[2] = v;
                dst[3] = To::opaque_alpha();
            }
            (4, 2) => {
                let v = src[0].convert();
                dst[0] = v;
                dst[1] = v;
                dst[2] = v;
                dst[3] = src[1].convert();
            }
            (4, 3) => {
                dst[0] = src[0].convert();
                dst[1] = src[1].convert();
                dst[2] = src[2].convert();
                dst[3] = To::opaque_alpha();
            }
            (4, 4) => {
                dst[0] = src[0].convert();
                dst[1] = src[1].convert();
                dst[2] = src[2].convert();
                dst[3] = src[3].convert();
            }

            _ => unreachable!(),
        }
    }
}

// =============================================================================
// Format dispatch helpers
// =============================================================================

/// Get the Rust type size and conversion function for a given channel size/type pair.
#[derive(Clone, Copy)]
pub(crate) struct ConversionInfo {
    pub from_channels: usize,
    pub to_channels: usize,
    pub from_size: ChannelSize,
    pub to_size: ChannelSize,
}

impl ConversionInfo {
    pub fn new(from_fmt: ColorFormat, to_fmt: ColorFormat) -> Self {
        Self {
            from_channels: from_fmt.channel_count.channel_count() as usize,
            to_channels: to_fmt.channel_count.channel_count() as usize,
            from_size: from_fmt.channel_size,
            to_size: to_fmt.channel_size,
        }
    }
}

/// Dispatch row conversion based on channel sizes.
/// This calls the appropriate generic convert_row_scalar with the right types.
pub(crate) fn dispatch_convert_row_scalar(
    from_row: &[u8],
    to_row: &mut [u8],
    width: usize,
    info: &ConversionInfo,
) {
    match (info.from_size, info.to_size) {
        (ChannelSize::_8bit, ChannelSize::_8bit) => {
            convert_row_scalar::<u8, u8>(
                from_row,
                to_row,
                width,
                info.from_channels,
                info.to_channels,
            );
        }
        (ChannelSize::_8bit, ChannelSize::_16bit) => {
            convert_row_scalar::<u8, u16>(
                from_row,
                to_row,
                width,
                info.from_channels,
                info.to_channels,
            );
        }
        (ChannelSize::_8bit, ChannelSize::_32bit) => {
            convert_row_scalar::<u8, f32>(
                from_row,
                to_row,
                width,
                info.from_channels,
                info.to_channels,
            );
        }
        (ChannelSize::_16bit, ChannelSize::_8bit) => {
            convert_row_scalar::<u16, u8>(
                from_row,
                to_row,
                width,
                info.from_channels,
                info.to_channels,
            );
        }
        (ChannelSize::_16bit, ChannelSize::_16bit) => {
            convert_row_scalar::<u16, u16>(
                from_row,
                to_row,
                width,
                info.from_channels,
                info.to_channels,
            );
        }
        (ChannelSize::_16bit, ChannelSize::_32bit) => {
            convert_row_scalar::<u16, f32>(
                from_row,
                to_row,
                width,
                info.from_channels,
                info.to_channels,
            );
        }
        (ChannelSize::_32bit, ChannelSize::_8bit) => {
            convert_row_scalar::<f32, u8>(
                from_row,
                to_row,
                width,
                info.from_channels,
                info.to_channels,
            );
        }
        (ChannelSize::_32bit, ChannelSize::_16bit) => {
            convert_row_scalar::<f32, u16>(
                from_row,
                to_row,
                width,
                info.from_channels,
                info.to_channels,
            );
        }
        (ChannelSize::_32bit, ChannelSize::_32bit) => {
            convert_row_scalar::<f32, f32>(
                from_row,
                to_row,
                width,
                info.from_channels,
                info.to_channels,
            );
        }
    }
}
