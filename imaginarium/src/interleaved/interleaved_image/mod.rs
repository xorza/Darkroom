use crate::color::color_format::ColorFormat;
use crate::color::sample::Sample;
use crate::error::{ByteLenError, PlanesError};
use crate::interleaved::conversion::convert_image;
use crate::interleaved::image_pixels::{ImagePixels, Stored};
use crate::interleaved::interleaved_desc::InterleavedDesc;
use crate::planar::planar_image::PlanarImage;
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

    /// An image of `desc` with every sample zero: black, and transparent where it has alpha.
    pub fn new_zeroed(desc: InterleavedDesc) -> Self {
        Self {
            pixels: ImagePixels::new_zeroed(desc.color_format(), desc.size()),
        }
    }

    /// An image over `bytes`, or why they are not `desc`'s pixels: another byte length. A `u8`
    /// format keeps the allocation.
    pub fn new_with_data(desc: InterleavedDesc, bytes: Vec<u8>) -> Result<Self, ByteLenError> {
        if bytes.len() != desc.byte_len() {
            return Err(ByteLenError {
                desc,
                byte_len: bytes.len(),
            });
        }
        Ok(Self {
            pixels: ImagePixels::from_bytes(desc.color_format(), desc.size(), bytes),
        })
    }

    /// An image over typed channel values, `T` being the format's sample type: the allocation
    /// is kept.
    ///
    /// # Panics
    /// If `T` is not the format's sample type, or `samples` are not `desc`'s.
    pub(crate) fn from_samples<T: Sample>(desc: InterleavedDesc, samples: Vec<T>) -> Self {
        assert_eq!(
            samples.len(),
            desc.sample_count(),
            "the samples of a {desc} image"
        );
        Self {
            pixels: ImagePixels::from_samples(desc.color_format(), desc.size(), samples),
        }
    }

    /// This image in `color_format`: itself when the format already matches.
    #[must_use]
    pub fn convert(self, color_format: ColorFormat) -> Self {
        if self.desc().color_format() == color_format {
            return self;
        }
        self.convert_to(color_format)
    }

    /// This image with its alpha channel dropped, in the same sample type: the one layout change
    /// that loses data, which a caller names. An image with no alpha is itself.
    #[must_use]
    pub fn without_alpha(self) -> Self {
        let format = self.desc().color_format().without_alpha();
        self.convert(format)
    }

    /// Borrowing counterpart of [`convert`](Self::convert): converts into a freshly allocated
    /// image, leaving `self` alone, so a caller that holds only a borrow skips the copy of the
    /// source that `convert`'s `self` receiver would force. The same format is a full copy.
    #[must_use]
    pub fn convert_to(&self, color_format: ColorFormat) -> Self {
        let source = self.desc();
        if source.color_format() == color_format {
            return self.clone();
        }
        let mut result = Self {
            pixels: ImagePixels::new_zeroed(color_format, source.size()),
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
    pub(crate) fn planes<T: Stored<N>, const N: usize>(
        &self,
    ) -> Result<[Buffer2<T>; N], PlanesError> {
        T::unwrap(&self.pixels)
            .map(Buffer2::deinterleave)
            .ok_or_else(|| planes_refused::<T, N>(self.desc()))
    }

    /// [`Self::planes`], taking the image: a one-channel image keeps its allocation.
    pub(crate) fn into_planes<T: Stored<N>, const N: usize>(
        self,
    ) -> Result<[Buffer2<T>; N], PlanesError> {
        let desc = self.desc();
        T::unwrap_owned(self.pixels)
            .map(Buffer2::into_planes)
            .ok_or_else(|| planes_refused::<T, N>(desc))
    }
}

impl InterleavedImage {
    /// The image as `f32` planes on the unit scale, its alpha channel dropped: each pixel read
    /// once, each sample by [`Sample::to_unit`], exactly what a conversion to `f32` gives.
    pub(crate) fn unit_planes(&self) -> PlanarImage {
        match &self.pixels {
            ImagePixels::MONO_U8(pixels) => mono_planes(pixels),
            ImagePixels::MONO_U16(pixels) => mono_planes(pixels),
            ImagePixels::MONO_F32(pixels) => mono_planes(pixels),
            ImagePixels::RGB_U8(pixels) => rgb_planes(pixels),
            ImagePixels::RGB_U16(pixels) => rgb_planes(pixels),
            ImagePixels::RGB_F32(pixels) => rgb_planes(pixels),
            ImagePixels::RGBA_U8(pixels) => rgb_planes(pixels),
            ImagePixels::RGBA_U16(pixels) => rgb_planes(pixels),
            ImagePixels::RGBA_F32(pixels) => rgb_planes(pixels),
        }
    }
}

/// The one channel of `pixels` as a unit `f32` plane.
fn mono_planes<T: Sample>(pixels: &Buffer2<[T; 1]>) -> PlanarImage {
    let [plane] = pixels.channel_planes(T::to_unit);
    PlanarImage::new_mono(plane)
}

/// The first three channels of `pixels` as unit `f32` planes.
fn rgb_planes<T: Sample, const N: usize>(pixels: &Buffer2<[T; N]>) -> PlanarImage {
    PlanarImage::new_rgb(pixels.channel_planes::<f32, 3>(T::to_unit))
}

/// Why an image of `desc` has no `N` planes of `T`.
const fn planes_refused<T: Stored<N>, const N: usize>(desc: InterleavedDesc) -> PlanesError {
    PlanesError::Mismatch {
        color_format: desc.color_format(),
        plane_count: N,
        sample_type: T::TYPE,
    }
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
                type Error = PlanesError;

                fn try_from(image: &InterleavedImage) -> Result<Self, PlanesError> {
                    image.planes()
                }
            }

            impl TryFrom<InterleavedImage> for [Buffer2<$t>; $n] {
                type Error = PlanesError;

                fn try_from(image: InterleavedImage) -> Result<Self, PlanesError> {
                    image.into_planes()
                }
            }
        )+
    };
}

planar_conversions!(1 u8, 1 u16, 1 f32, 3 u8, 3 u16, 3 f32, 4 u8, 4 u16, 4 f32);

#[cfg(test)]
mod tests;
