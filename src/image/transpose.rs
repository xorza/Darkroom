//! Interleave ⟷ deinterleave — the layout transpose between the interleaved
//! [`ImageData`] / [`Image`] and the planar [`DeinterleavedImageData`]. The
//! format is unchanged; only the byte order moves (pixel-major ⟷ channel-major),
//! and each direction is one rayon-parallel copy. (Changing the *format* is a
//! different operation — see [`conversion`](crate::image::conversion).)
//!
//! Two granularities, the runtime one built on the typed one:
//! - typed `ImageData<N, T>` ⟷ `DeinterleavedImageData<N, T>` — the gather /
//!   scatter that knows the interleaving.
//! - runtime [`Image`] ⟷ `DeinterleavedImageData<N, T>` — borrowing, so a caller
//!   (e.g. lumos) never clones the whole image; deinterleaving is fallible (the
//!   image's format must match `N`/`T`, convert it first otherwise).

use rayon::prelude::*;

use crate::common::buffer2::Buffer2;
use crate::common::color_format::ColorFormat;
use crate::common::error::{Error, Result};
use crate::image::image_data::deinterleaved::DeinterleavedImageData;
use crate::image::image_data::interleaved::{AnyImageData, ImageData};
use crate::image::{Image, ImageDesc};

/// Interleave `N` channel planes into one `[T; N]`-per-pixel buffer.
impl<const N: usize, T: bytemuck::Pod + Send + Sync> From<&DeinterleavedImageData<N, T>>
    for ImageData<N, T>
{
    fn from(planar: &DeinterleavedImageData<N, T>) -> Self {
        let width = planar.width();
        let height = planar.height();
        let planes: [&[T]; N] = std::array::from_fn(|c| planar.channels[c].pixels());

        let mut pixels = vec![[T::zeroed(); N]; width * height];
        pixels.par_iter_mut().enumerate().for_each(|(i, pixel)| {
            for (c, plane) in planes.iter().enumerate() {
                pixel[c] = plane[i];
            }
        });

        ImageData::from_buffer(Buffer2::new(width, height, pixels))
    }
}

/// Deinterleave an `[T; N]`-per-pixel buffer into `N` channel planes.
impl<const N: usize, T: bytemuck::Pod + Send + Sync> From<&ImageData<N, T>>
    for DeinterleavedImageData<N, T>
{
    fn from(interleaved: &ImageData<N, T>) -> Self {
        let width = interleaved.width();
        let height = interleaved.height();
        let pixels = interleaved.buffer.pixels();

        let channels: [Buffer2<T>; N] = std::array::from_fn(|c| {
            let mut plane = vec![T::zeroed(); width * height];
            plane
                .par_iter_mut()
                .enumerate()
                .for_each(|(i, sample)| *sample = pixels[i][c]);
            Buffer2::new(width, height, plane)
        });

        DeinterleavedImageData::from_channels(channels)
    }
}

/// Generate the borrow-based `Image ⟷ DeinterleavedImageData<N, T>` pair for one
/// concrete format. The variant + `ColorFormat` are spelled out so the typed
/// `ImageData<N, T>` maps to exactly one `AnyImageData` arm.
macro_rules! impl_image_planar_conv {
    ($n:literal, $t:ty, $variant:ident, $format:expr) => {
        impl From<&DeinterleavedImageData<$n, $t>> for Image {
            fn from(planar: &DeinterleavedImageData<$n, $t>) -> Self {
                let interleaved: ImageData<$n, $t> = planar.into();
                let desc = ImageDesc::new(interleaved.width(), interleaved.height(), $format);
                Image {
                    desc,
                    data: AnyImageData::$variant(interleaved),
                }
            }
        }

        impl TryFrom<&Image> for DeinterleavedImageData<$n, $t> {
            type Error = Error;

            fn try_from(image: &Image) -> Result<Self> {
                match &image.data {
                    AnyImageData::$variant(interleaved) => Ok(interleaved.into()),
                    _ => Err(Error::InvalidColorFormat(format!(
                        "cannot deinterleave a {} image into DeinterleavedImageData<{}, {}>",
                        image.desc.color_format,
                        $n,
                        stringify!($t),
                    ))),
                }
            }
        }
    };
}

impl_image_planar_conv!(1, u8, L_U8, ColorFormat::L_U8);
impl_image_planar_conv!(1, u16, L_U16, ColorFormat::L_U16);
impl_image_planar_conv!(1, f32, L_F32, ColorFormat::L_F32);
impl_image_planar_conv!(3, u8, RGB_U8, ColorFormat::RGB_U8);
impl_image_planar_conv!(3, u16, RGB_U16, ColorFormat::RGB_U16);
impl_image_planar_conv!(3, f32, RGB_F32, ColorFormat::RGB_F32);
impl_image_planar_conv!(4, u8, RGBA_U8, ColorFormat::RGBA_U8);
impl_image_planar_conv!(4, u16, RGBA_U16, ColorFormat::RGBA_U16);
impl_image_planar_conv!(4, f32, RGBA_F32, ColorFormat::RGBA_F32);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interleave_gathers_planes_into_pixels() {
        // RGB 2x1: R = [1, 4], G = [2, 5], B = [3, 6] → pixels [[1,2,3], [4,5,6]].
        let planar = DeinterleavedImageData::from_channels([
            Buffer2::new(2, 1, vec![1u8, 4]),
            Buffer2::new(2, 1, vec![2u8, 5]),
            Buffer2::new(2, 1, vec![3u8, 6]),
        ]);
        let interleaved: ImageData<3, u8> = (&planar).into();
        assert_eq!(interleaved.buffer.pixels(), &[[1, 2, 3], [4, 5, 6]]);
    }

    #[test]
    fn deinterleave_scatters_pixels_into_planes() {
        let interleaved = ImageData::<3, u8>::from_bytes(2, 1, &[1, 2, 3, 4, 5, 6]);
        let planar: DeinterleavedImageData<3, u8> = (&interleaved).into();
        assert_eq!(planar.channels[0].pixels(), &[1, 4]);
        assert_eq!(planar.channels[1].pixels(), &[2, 5]);
        assert_eq!(planar.channels[2].pixels(), &[3, 6]);
    }

    #[test]
    fn typed_round_trip_preserves_data() {
        #[rustfmt::skip]
        let samples = [
            0.1f32, 0.2, 0.3,   0.4, 0.5, 0.6,
            0.7,    0.8, 0.9,   1.0, 1.1, 1.2,
        ];
        let interleaved = ImageData::<3, f32>::from_bytes(2, 2, bytemuck::cast_slice(&samples));
        let planar: DeinterleavedImageData<3, f32> = (&interleaved).into();
        // Planes hold the per-channel columns…
        assert_eq!(planar.channels[0].pixels(), &[0.1, 0.4, 0.7, 1.0]);
        assert_eq!(planar.channels[1].pixels(), &[0.2, 0.5, 0.8, 1.1]);
        assert_eq!(planar.channels[2].pixels(), &[0.3, 0.6, 0.9, 1.2]);
        // …and round-trip back to the original interleaving.
        let back: ImageData<3, f32> = (&planar).into();
        assert_eq!(back.buffer.pixels(), interleaved.buffer.pixels());
    }

    #[test]
    fn single_channel_round_trips() {
        let interleaved =
            ImageData::<1, u16>::from_bytes(3, 1, bytemuck::cast_slice(&[10u16, 20, 30]));
        let planar: DeinterleavedImageData<1, u16> = (&interleaved).into();
        assert_eq!(planar.channels[0].pixels(), &[10, 20, 30]);
        let back: ImageData<1, u16> = (&planar).into();
        assert_eq!(back.buffer.pixels(), &[[10], [20], [30]]);
    }

    #[test]
    fn image_from_planar_interleaves_and_tags_format() {
        let planar = DeinterleavedImageData::from_channels([
            Buffer2::new(2, 1, vec![1.0f32, 4.0]),
            Buffer2::new(2, 1, vec![2.0f32, 5.0]),
            Buffer2::new(2, 1, vec![3.0f32, 6.0]),
        ]);
        let image = Image::from(&planar);
        assert_eq!(image.desc.color_format, ColorFormat::RGB_F32);
        assert_eq!((image.desc.width, image.desc.height), (2, 1));
        let samples: &[f32] = bytemuck::cast_slice(image.bytes());
        assert_eq!(samples, &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]); // RGBRGB
    }

    #[test]
    fn planar_from_image_deinterleaves() {
        let image = Image::new_with_data(
            ImageDesc::new(2, 1, ColorFormat::RGB_U8),
            vec![1, 2, 3, 4, 5, 6],
        )
        .unwrap();
        let planar: DeinterleavedImageData<3, u8> = (&image).try_into().unwrap();
        assert_eq!(planar.channels[0].pixels(), &[1, 4]);
        assert_eq!(planar.channels[1].pixels(), &[2, 5]);
        assert_eq!(planar.channels[2].pixels(), &[3, 6]);
    }

    #[test]
    fn round_trip_image_planar_image_is_identity() {
        let image = Image::new_with_data(
            ImageDesc::new(2, 2, ColorFormat::RGBA_U8),
            (0..16).collect(),
        )
        .unwrap();
        let bytes = image.bytes().to_vec();
        let planar: DeinterleavedImageData<4, u8> = (&image).try_into().unwrap();
        let back = Image::from(&planar);
        assert_eq!(back.bytes(), &bytes[..]);
        assert_eq!(back.desc, image.desc);
    }

    #[test]
    fn deinterleave_into_mismatched_format_errors() {
        let image = Image::new_black(ImageDesc::new(2, 2, ColorFormat::RGB_U8)).unwrap();
        // RGB_U8 image can't be deinterleaved as 1-channel f32…
        let result: Result<DeinterleavedImageData<1, f32>> = (&image).try_into();
        assert!(result.is_err());
        // …but matching the format succeeds.
        let ok: Result<DeinterleavedImageData<3, u8>> = (&image).try_into();
        assert!(ok.is_ok());
    }
}
