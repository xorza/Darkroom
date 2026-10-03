pub(crate) mod conversion;
pub(crate) mod file_format;
pub(crate) mod image_desc;
pub(crate) mod image_pixels;
mod io;

use std::path::Path;

use crate::common::buffer2::Buffer2;
use crate::common::color_format::ColorFormat;
use crate::common::error::{Error, Result};
use crate::common::sample::Sample;
use crate::image::conversion::convert_image;
use crate::image::file_format::FileFormat;
use crate::image::image_desc::ImageDesc;
use crate::image::image_pixels::{ImagePixels, Stored};

/// A runtime-format image backed by tightly packed, typed interleaved pixels.
#[derive(Clone, Debug)]
pub struct Image {
    pixels: ImagePixels,
}

impl Image {
    /// Dimensions and format derived from the owned typed storage.
    #[inline]
    pub const fn desc(&self) -> ImageDesc {
        ImageDesc::new(
            self.pixels.width(),
            self.pixels.height(),
            self.pixels.format(),
        )
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

    pub fn new_black(desc: ImageDesc) -> Result<Image> {
        desc.validate()?;
        let pixels = ImagePixels::new_zeroed(desc.color_format, desc.width, desc.height);
        Ok(Image { pixels })
    }

    /// An image over `bytes`, which must hold exactly the descriptor's pixels. A `u8` format
    /// keeps the allocation.
    pub fn new_with_data(desc: ImageDesc, bytes: Vec<u8>) -> Result<Image> {
        desc.validate()?;
        if bytes.len() != desc.size_in_bytes() {
            return Err(Error::SizeMismatch(format!(
                "bytes length {} does not match expected size {}",
                bytes.len(),
                desc.size_in_bytes()
            )));
        }
        let pixels = ImagePixels::from_bytes(desc.color_format, desc.width, desc.height, bytes);
        Ok(Image { pixels })
    }

    /// An image over typed channel values, `T` being the format's sample type: the allocation
    /// is kept.
    ///
    /// # Panics
    /// If `T` is not the format's sample type.
    pub(crate) fn from_samples<T: Sample>(desc: ImageDesc, samples: Vec<T>) -> Result<Image> {
        desc.validate()?;
        let expected = desc.width * desc.height * desc.color_format.channel_count.count();
        if samples.len() != expected {
            return Err(Error::SizeMismatch(format!(
                "{} samples do not make a {desc} image of {expected}",
                samples.len()
            )));
        }
        let pixels = ImagePixels::from_samples(desc.color_format, desc.width, desc.height, samples);
        Ok(Image { pixels })
    }

    pub fn read_file<P: AsRef<Path>>(filename: P) -> Result<Image> {
        let filename = filename.as_ref();
        match FileFormat::from_path(filename)? {
            FileFormat::Png | FileFormat::Jpeg => io::load_png_jpeg(filename),
            FileFormat::Tiff => io::load_tiff(filename),
        }
    }

    pub fn save_file<P: AsRef<Path>>(&self, filename: P) -> Result<()> {
        let filename = filename.as_ref();
        match FileFormat::from_path(filename)? {
            FileFormat::Png => io::save_png(self, filename),
            FileFormat::Jpeg => io::save_jpg(self, filename),
            FileFormat::Tiff => io::save_tiff(self, filename),
        }
    }

    /// This image in `color_format`: itself when the format already matches.
    #[must_use]
    pub fn convert(self, color_format: ColorFormat) -> Image {
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
    pub fn convert_to(&self, color_format: ColorFormat) -> Image {
        let source = self.desc();
        if source.color_format == color_format {
            return self.clone();
        }
        let mut result = Image {
            pixels: ImagePixels::new_zeroed(color_format, source.width, source.height),
        };
        convert_image(self, &mut result);
        result
    }
}

impl Image {
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
        Image {
            pixels: T::wrap(pixels),
        }
    }

    /// The `N` channel planes of an image whose format is `N` channels of `T`.
    fn planes<T: Stored<N>, const N: usize>(&self) -> Result<[Buffer2<T>; N]> {
        T::unwrap(&self.pixels)
            .map(Buffer2::deinterleave)
            .ok_or_else(|| {
                Error::InvalidColorFormat(format!(
                    "cannot deinterleave a {} image into {N} {} planes",
                    self.desc().color_format,
                    T::TYPE,
                ))
            })
    }
}

/// The public face of [`Image::from_planes`] and [`Image::planes`] for one format, spelled per
/// type because the bound that unifies them names crate-private storage.
macro_rules! planar_conversions {
    ($($n:literal $t:ty),+ $(,)?) => {
        $(
            impl From<[&Buffer2<$t>; $n]> for Image {
                fn from(planes: [&Buffer2<$t>; $n]) -> Self {
                    Image::from_planes(planes)
                }
            }

            impl TryFrom<&Image> for [Buffer2<$t>; $n] {
                type Error = Error;

                fn try_from(image: &Image) -> Result<Self> {
                    image.planes()
                }
            }
        )+
    };
}

planar_conversions!(1 u8, 1 u16, 1 f32, 3 u8, 3 u16, 3 f32, 4 u8, 4 u16, 4 f32);

#[cfg(test)]
mod tests;
