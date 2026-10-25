pub(crate) mod conversion;
pub(crate) mod image_desc;
pub(crate) mod image_pixels;
mod io;
pub(crate) mod raster_format;

use std::io::{BufRead, Seek, Write};

use crate::color::color_format::ColorFormat;
use crate::color::sample::Sample;
use crate::error::{Error, Result};
use crate::image::conversion::convert_image;
use crate::image::image_desc::InterleavedDesc;
use crate::image::image_pixels::{ImagePixels, Stored};
use crate::image::raster_format::RasterFormat;
use common::Buffer2;

/// A runtime-format image backed by tightly packed, typed interleaved pixels.
#[derive(Clone, Debug)]
pub struct InterleavedImage {
    pixels: ImagePixels,
}

impl InterleavedImage {
    /// Dimensions and format derived from the owned typed storage.
    #[inline]
    pub const fn desc(&self) -> InterleavedDesc {
        InterleavedDesc::new(self.pixels.size(), self.pixels.format())
    }

    /// The interleaved pixel bytes — a zero-copy `&[u8]` view of the typed buffer.
    pub fn bytes(&self) -> &[u8] {
        self.pixels.bytes()
    }

    /// The pixel bytes as an owned `Vec<u8>`. A `u8` format hands over its allocation; `u16`
    /// and `f32` storage is aligned for its type, which a `Vec<u8>` cannot carry, so it copies.
    pub fn into_bytes(self) -> Vec<u8> {
        self.pixels.into_bytes()
    }

    /// The interleaved pixel bytes, mutable — zero-copy; writes hit the buffer.
    pub fn bytes_mut(&mut self) -> &mut [u8] {
        self.pixels.bytes_mut()
    }

    pub fn new_black(desc: InterleavedDesc) -> Result<Self> {
        desc.validate()?;
        let pixels = ImagePixels::new_zeroed(desc.color_format, desc.size);
        Ok(Self { pixels })
    }

    /// An image over `bytes`, which must hold exactly the descriptor's pixels. A `u8` format
    /// keeps the allocation.
    pub fn new_with_data(desc: InterleavedDesc, bytes: Vec<u8>) -> Result<Self> {
        desc.validate()?;
        if bytes.len() != desc.byte_len() {
            return Err(Error::SizeMismatch(format!(
                "bytes length {} does not match expected size {}",
                bytes.len(),
                desc.byte_len()
            )));
        }
        let pixels = ImagePixels::from_bytes(desc.color_format, desc.size, bytes);
        Ok(Self { pixels })
    }

    /// An image over typed channel values, `T` being the format's sample type: the allocation
    /// is kept.
    ///
    /// # Panics
    /// If `T` is not the format's sample type.
    pub(crate) fn from_samples<T: Sample>(desc: InterleavedDesc, samples: Vec<T>) -> Result<Self> {
        desc.validate()?;
        let expected = desc.size.pixel_count() * desc.color_format.channel_count.count();
        if samples.len() != expected {
            return Err(Error::SizeMismatch(format!(
                "{} samples do not make a {desc} image of {expected}",
                samples.len()
            )));
        }
        let pixels = ImagePixels::from_samples(desc.color_format, desc.size, samples);
        Ok(Self { pixels })
    }

    /// The image `input` holds, in `format`, decoded as it is read: a file whoever owns the
    /// reading opened, or bytes in a [`Cursor`](std::io::Cursor).
    pub fn decode(mut input: impl BufRead + Seek, format: RasterFormat) -> Result<Self> {
        io::load(&mut input, format)
    }

    /// This image in `format`, written into `out`, which the encoder may seek back over.
    pub fn encode(&self, mut out: impl Write + Seek, format: RasterFormat) -> Result<()> {
        io::save(self, format, &mut out)
    }

    /// This image in `color_format`: itself when the format already matches.
    #[must_use]
    pub fn convert(self, color_format: ColorFormat) -> Self {
        if self.desc().color_format == color_format {
            return self;
        }
        self.convert_to(color_format)
    }

    /// Borrowing counterpart of [`convert`](Self::convert): converts into a freshly
    /// allocated image, leaving `self` alone — a caller that only holds a view (e.g.
    /// a CPU borrow of an `ImageBuffer`) skips the source deep-copy that `convert`'s
    /// `self` receiver would force. The same format is a full copy.
    #[must_use]
    pub fn convert_to(&self, color_format: ColorFormat) -> Self {
        let source = self.desc();
        if source.color_format == color_format {
            return self.clone();
        }
        let mut result = Self {
            pixels: ImagePixels::new_zeroed(color_format, source.size),
        };
        convert_image(self, &mut result);
        result
    }
}

impl InterleavedImage {
    /// Interleaves `N` channel planes into an image of the format they spell.
    ///
    /// # Panics
    /// Unless the planes share dimensions and hold at least one pixel.
    fn from_planes<T: Stored<N>, const N: usize>(planes: [&Buffer2<T>; N]) -> Self {
        let pixels = Buffer2::interleave(planes);
        assert!(
            pixels.width() > 0 && pixels.height() > 0,
            "an image needs at least one pixel"
        );
        Self {
            pixels: T::wrap(pixels),
        }
    }

    /// The `N` channel planes of an image whose format is `N` channels of `T`.
    fn planes<T: Stored<N>, const N: usize>(&self) -> Result<[Buffer2<T>; N]> {
        T::unwrap(&self.pixels)
            .map(Buffer2::deinterleave)
            .ok_or_else(|| planes_refused::<T, N>(self.desc()))
    }

    /// [`Self::planes`], taking the image: a one-channel image keeps its allocation.
    fn into_planes<T: Stored<N>, const N: usize>(self) -> Result<[Buffer2<T>; N]> {
        let desc = self.desc();
        T::unwrap_owned(self.pixels)
            .map(Buffer2::into_planes)
            .ok_or_else(|| planes_refused::<T, N>(desc))
    }
}

/// Why an image of `desc` has no `N` planes of `T`.
fn planes_refused<T: Stored<N>, const N: usize>(desc: InterleavedDesc) -> Error {
    Error::InvalidColorFormat(format!(
        "cannot deinterleave a {} image into {N} {} planes",
        desc.color_format,
        T::TYPE,
    ))
}

/// The public face of [`InterleavedImage::from_planes`], [`InterleavedImage::planes`] and
/// [`InterleavedImage::into_planes`] for one format, spelled per type because the bound that
/// unifies them names crate-private storage.
macro_rules! planar_conversions {
    ($($n:literal $t:ty),+ $(,)?) => {
        $(
            impl From<[&Buffer2<$t>; $n]> for InterleavedImage {
                fn from(planes: [&Buffer2<$t>; $n]) -> Self {
                    InterleavedImage::from_planes(planes)
                }
            }

            impl TryFrom<&InterleavedImage> for [Buffer2<$t>; $n] {
                type Error = Error;

                fn try_from(image: &InterleavedImage) -> Result<Self> {
                    image.planes()
                }
            }

            impl TryFrom<InterleavedImage> for [Buffer2<$t>; $n] {
                type Error = Error;

                fn try_from(image: InterleavedImage) -> Result<Self> {
                    image.into_planes()
                }
            }
        )+
    };
}

planar_conversions!(1 u8, 1 u16, 1 f32, 3 u8, 3 u16, 3 f32, 4 u8, 4 u16, 4 f32);

#[cfg(feature = "internals")]
pub(crate) mod bench;
#[cfg(test)]
mod tests;
