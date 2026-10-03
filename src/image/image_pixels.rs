//! Runtime-format interleaved storage owned by [`Image`](crate::image::Image).

use crate::common::buffer2::Buffer2;
use crate::common::color_format::{ChannelCount, ColorFormat, SampleType};
use crate::common::sample::Sample;

/// Tightly packed pixels of one of the nine formats, typed.
#[derive(Debug, Clone)]
#[expect(
    non_camel_case_types,
    reason = "each variant is named for the `ColorFormat` constant it stores"
)]
pub(crate) enum ImagePixels {
    L_U8(Buffer2<[u8; 1]>),
    L_U16(Buffer2<[u16; 1]>),
    L_F32(Buffer2<[f32; 1]>),
    RGB_U8(Buffer2<[u8; 3]>),
    RGB_U16(Buffer2<[u16; 3]>),
    RGB_F32(Buffer2<[f32; 3]>),
    RGBA_U8(Buffer2<[u8; 4]>),
    RGBA_U16(Buffer2<[u16; 4]>),
    RGBA_F32(Buffer2<[f32; 4]>),
}

/// The variant a typed pixel buffer is stored in, and back.
pub(crate) trait Stored<const N: usize>: Sample {
    fn wrap(pixels: Buffer2<[Self; N]>) -> ImagePixels;
    fn unwrap(pixels: &ImagePixels) -> Option<&Buffer2<[Self; N]>>;
}

/// Implements [`Stored`] for one variant.
macro_rules! stored {
    ($n:literal, $t:ty, $variant:ident) => {
        impl Stored<$n> for $t {
            fn wrap(pixels: Buffer2<[Self; $n]>) -> ImagePixels {
                ImagePixels::$variant(pixels)
            }

            fn unwrap(pixels: &ImagePixels) -> Option<&Buffer2<[Self; $n]>> {
                match pixels {
                    ImagePixels::$variant(pixels) => Some(pixels),
                    _ => None,
                }
            }
        }
    };
}

stored!(1, u8, L_U8);
stored!(1, u16, L_U16);
stored!(1, f32, L_F32);
stored!(3, u8, RGB_U8);
stored!(3, u16, RGB_U16);
stored!(3, f32, RGB_F32);
stored!(4, u8, RGBA_U8);
stored!(4, u16, RGBA_U16);
stored!(4, f32, RGBA_F32);

/// Runs `$body` with `$pixels` bound to whichever typed buffer `$self` holds.
macro_rules! with_pixels {
    ($self:expr, $pixels:ident => $body:expr) => {
        match $self {
            ImagePixels::L_U8($pixels) => $body,
            ImagePixels::L_U16($pixels) => $body,
            ImagePixels::L_F32($pixels) => $body,
            ImagePixels::RGB_U8($pixels) => $body,
            ImagePixels::RGB_U16($pixels) => $body,
            ImagePixels::RGB_F32($pixels) => $body,
            ImagePixels::RGBA_U8($pixels) => $body,
            ImagePixels::RGBA_U16($pixels) => $body,
            ImagePixels::RGBA_F32($pixels) => $body,
        }
    };
}

/// Builds the variant of one format: [`build_for`] picks the types, the builder the contents.
trait Build {
    fn build<T: Stored<N>, const N: usize>(self) -> ImagePixels;
}

fn build_for(format: ColorFormat, builder: impl Build) -> ImagePixels {
    match (format.channel_count, format.sample_type) {
        (ChannelCount::L, SampleType::U8) => builder.build::<u8, 1>(),
        (ChannelCount::L, SampleType::U16) => builder.build::<u16, 1>(),
        (ChannelCount::L, SampleType::F32) => builder.build::<f32, 1>(),
        (ChannelCount::Rgb, SampleType::U8) => builder.build::<u8, 3>(),
        (ChannelCount::Rgb, SampleType::U16) => builder.build::<u16, 3>(),
        (ChannelCount::Rgb, SampleType::F32) => builder.build::<f32, 3>(),
        (ChannelCount::Rgba, SampleType::U8) => builder.build::<u8, 4>(),
        (ChannelCount::Rgba, SampleType::U16) => builder.build::<u16, 4>(),
        (ChannelCount::Rgba, SampleType::F32) => builder.build::<f32, 4>(),
    }
}

/// All-zero pixels.
#[derive(Debug)]
struct Zeroed {
    width: usize,
    height: usize,
}

impl Build for Zeroed {
    fn build<T: Stored<N>, const N: usize>(self) -> ImagePixels {
        T::wrap(Buffer2::new_filled(
            self.width,
            self.height,
            [T::zeroed(); N],
        ))
    }
}

/// Pixels taken from bytes: a `u8` format keeps the allocation; `u16` and `f32` copy into
/// storage aligned for their type.
#[derive(Debug)]
struct FromBytes {
    width: usize,
    height: usize,
    bytes: Vec<u8>,
}

impl Build for FromBytes {
    fn build<T: Stored<N>, const N: usize>(self) -> ImagePixels {
        let pixels = bytemuck::try_cast_vec::<u8, [T; N]>(self.bytes)
            .unwrap_or_else(|(_, bytes)| bytemuck::pod_collect_to_vec(&bytes));
        T::wrap(Buffer2::new(self.width, self.height, pixels))
    }
}

/// Pixels taken from typed samples, keeping their allocation.
#[derive(Debug)]
struct FromSamples<T> {
    width: usize,
    height: usize,
    samples: Vec<T>,
}

impl<T: Sample> Build for FromSamples<T> {
    fn build<U: Stored<N>, const N: usize>(self) -> ImagePixels {
        let pixels = bytemuck::try_cast_vec::<T, [U; N]>(self.samples)
            .map_err(|(error, _)| error)
            .expect("the samples are of the format's sample type, whole pixels of it");
        U::wrap(Buffer2::new(self.width, self.height, pixels))
    }
}

impl ImagePixels {
    /// Takes `samples`, which must be of `format`'s sample type and hold exactly
    /// `width · height` of its pixels.
    pub(crate) fn from_samples<T: Sample>(
        format: ColorFormat,
        width: usize,
        height: usize,
        samples: Vec<T>,
    ) -> Self {
        assert_eq!(
            format.sample_type,
            T::TYPE,
            "samples of another type than {format}"
        );
        build_for(
            format,
            FromSamples {
                width,
                height,
                samples,
            },
        )
    }

    pub(crate) fn new_zeroed(format: ColorFormat, width: usize, height: usize) -> Self {
        build_for(format, Zeroed { width, height })
    }

    /// Takes `bytes`, which must hold exactly `width · height` pixels of `format`.
    pub(crate) fn from_bytes(
        format: ColorFormat,
        width: usize,
        height: usize,
        bytes: Vec<u8>,
    ) -> Self {
        build_for(
            format,
            FromBytes {
                width,
                height,
                bytes,
            },
        )
    }

    pub(crate) const fn format(&self) -> ColorFormat {
        match self {
            Self::L_U8(_) => ColorFormat::L_U8,
            Self::L_U16(_) => ColorFormat::L_U16,
            Self::L_F32(_) => ColorFormat::L_F32,
            Self::RGB_U8(_) => ColorFormat::RGB_U8,
            Self::RGB_U16(_) => ColorFormat::RGB_U16,
            Self::RGB_F32(_) => ColorFormat::RGB_F32,
            Self::RGBA_U8(_) => ColorFormat::RGBA_U8,
            Self::RGBA_U16(_) => ColorFormat::RGBA_U16,
            Self::RGBA_F32(_) => ColorFormat::RGBA_F32,
        }
    }

    pub(crate) const fn width(&self) -> usize {
        with_pixels!(self, pixels => pixels.width())
    }

    pub(crate) const fn height(&self) -> usize {
        with_pixels!(self, pixels => pixels.height())
    }

    pub(crate) fn bytes(&self) -> &[u8] {
        with_pixels!(self, pixels => bytemuck::cast_slice(pixels.pixels()))
    }

    pub(crate) fn bytes_mut(&mut self) -> &mut [u8] {
        with_pixels!(self, pixels => bytemuck::cast_slice_mut(pixels.pixels_mut()))
    }

    /// The pixel bytes, keeping the allocation of a `u8` format.
    pub(crate) fn into_bytes(self) -> Vec<u8> {
        fn bytes<T: bytemuck::Pod, const N: usize>(pixels: Buffer2<[T; N]>) -> Vec<u8> {
            bytemuck::try_cast_vec(pixels.into_vec())
                .unwrap_or_else(|(_, pixels)| bytemuck::cast_slice(&pixels).to_vec())
        }
        with_pixels!(self, pixels => bytes(pixels))
    }
}
