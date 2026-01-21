use std::mem::size_of;

use bytemuck::Pod;
use num_traits::Bounded;
use rayon::prelude::*;

use crate::common::color_format::*;
use crate::common::error::Result;
use crate::image::Image;

// =============================================================================
// Internal trait for channel conversion
// =============================================================================

pub(crate) trait ChannelConvert<To>: Copy {
    fn convert(self) -> To;
}

pub(crate) trait ChannelAverage: Copy {
    fn average3(a: Self, b: Self, c: Self) -> Self;
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
// Implement ChannelAverage
// =============================================================================

macro_rules! impl_average_int {
    ($t:ty, $wider:ty) => {
        impl ChannelAverage for $t {
            #[inline]
            fn average3(a: Self, b: Self, c: Self) -> Self {
                ((a as $wider + b as $wider + c as $wider) / 3) as $t
            }
        }
    };
}

macro_rules! impl_average_float {
    ($t:ty) => {
        impl ChannelAverage for $t {
            #[inline]
            fn average3(a: Self, b: Self, c: Self) -> Self {
                (a + b + c) / 3.0
            }
        }
    };
}

impl_average_int!(u8, u16);
impl_average_int!(u16, u32);
impl_average_float!(f32);

// =============================================================================
// Image conversion using traits (scalar implementation)
// =============================================================================

macro_rules! dispatch_to {
    ($from:expr, $to:expr, $from_rust:ty, { $($to_type:ident => $to_rust:ty),+ }) => {
        match $to.desc().color_format.channel_type {
            $(ChannelType::$to_type => convert_pixels::<$from_rust, $to_rust>($from, $to),)+
            #[allow(unreachable_patterns)]
            _ => unreachable!("Unsupported channel type conversion"),
        }
    };
}

macro_rules! dispatch_from {
    ($from:expr, $to:expr, { $($from_type:ident => $from_rust:ty),+ }, $to_types:tt) => {
        match $from.desc().color_format.channel_type {
            $(ChannelType::$from_type => dispatch_to!($from, $to, $from_rust, $to_types),)+
            #[allow(unreachable_patterns)]
            _ => unreachable!("Unsupported channel type conversion"),
        }
    };
}

pub(crate) fn convert_image(from: &Image, to: &mut Image) -> Result<()> {
    let from_size = from.desc().color_format.channel_size;
    let to_size = to.desc().color_format.channel_size;

    match (from_size, to_size) {
        (ChannelSize::_8bit, ChannelSize::_8bit) => dispatch_from!(from, to,
            { UInt => u8 },
            { UInt => u8 }
        ),
        (ChannelSize::_8bit, ChannelSize::_16bit) => dispatch_from!(from, to,
            { UInt => u8 },
            { UInt => u16 }
        ),
        (ChannelSize::_8bit, ChannelSize::_32bit) => dispatch_from!(from, to,
            { UInt => u8 },
            { Float => f32 }
        ),
        (ChannelSize::_16bit, ChannelSize::_8bit) => dispatch_from!(from, to,
            { UInt => u16 },
            { UInt => u8 }
        ),
        (ChannelSize::_16bit, ChannelSize::_16bit) => dispatch_from!(from, to,
            { UInt => u16 },
            { UInt => u16 }
        ),
        (ChannelSize::_16bit, ChannelSize::_32bit) => dispatch_from!(from, to,
            { UInt => u16 },
            { Float => f32 }
        ),
        (ChannelSize::_32bit, ChannelSize::_8bit) => dispatch_from!(from, to,
            { Float => f32 },
            { UInt => u8 }
        ),
        (ChannelSize::_32bit, ChannelSize::_16bit) => dispatch_from!(from, to,
            { Float => f32 },
            { UInt => u16 }
        ),
        (ChannelSize::_32bit, ChannelSize::_32bit) => dispatch_from!(from, to,
            { Float => f32 },
            { Float => f32 }
        ),
    }

    Ok(())
}

fn convert_pixels<From, To>(from: &Image, to: &mut Image)
where
    From: Pod + ChannelConvert<To> + ChannelAverage + Sync,
    To: Pod + Bounded + Send,
{
    debug_assert_eq!(from.desc().width, to.desc().width);
    debug_assert_eq!(from.desc().height, to.desc().height);
    debug_assert_eq!(
        from.desc().color_format.channel_size.byte_count() as usize,
        size_of::<From>()
    );
    debug_assert_eq!(
        to.desc().color_format.channel_size.byte_count() as usize,
        size_of::<To>()
    );

    if from.desc().color_format == to.desc().color_format {
        return;
    }

    let width = from.desc().width as usize;
    let to_channels = to.desc().color_format.channel_count.channel_count() as usize;
    let from_channels = from.desc().color_format.channel_count.channel_count() as usize;
    let from_stride = from.desc().stride;
    let to_stride = to.desc().stride;
    let from_row_bytes = width * from_channels * size_of::<From>();
    let to_row_bytes = width * to_channels * size_of::<To>();

    to.bytes_mut()
        .par_chunks_mut(to_stride)
        .enumerate()
        .for_each(|(y, to_row)| {
            let from_row = &from.bytes()[y * from_stride..];
            let from_row: &[From] = bytemuck::cast_slice(&from_row[..from_row_bytes]);
            let to_row: &mut [To] = bytemuck::cast_slice_mut(&mut to_row[..to_row_bytes]);

            for x in 0..width {
                let src = &from_row[x * from_channels..];
                let dst = &mut to_row[x * to_channels..];

                match (to_channels, from_channels) {
                    (1, 1) => dst[0] = src[0].convert(),
                    (1, 2) => dst[0] = src[0].convert(),
                    (1, 3) => dst[0] = From::average3(src[0], src[1], src[2]).convert(),
                    (1, 4) => dst[0] = From::average3(src[0], src[1], src[2]).convert(),

                    (2, 1) => {
                        dst[0] = src[0].convert();
                        dst[1] = To::max_value();
                    }
                    (2, 2) => {
                        dst[0] = src[0].convert();
                        dst[1] = src[1].convert();
                    }
                    (2, 3) => {
                        dst[0] = From::average3(src[0], src[1], src[2]).convert();
                        dst[1] = To::max_value();
                    }
                    (2, 4) => {
                        dst[0] = From::average3(src[0], src[1], src[2]).convert();
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
                        dst[3] = To::max_value();
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
                        dst[3] = To::max_value();
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
        });
}
