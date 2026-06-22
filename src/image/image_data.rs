//! Experimental **planar** image representation — not wired into the pipeline.
//!
//! Where [`Image`](super::Image) stores pixels interleaved in one byte buffer
//! (`RGBARGBA…`) with a runtime [`ColorFormat`](crate::common::color_format::ColorFormat),
//! [`ImageData`] holds the image deinterleaved as one `Buffer2<T>` per channel,
//! with the element type `T` and channel count `N` both carried in the type.
//!
//! This is a sketch to evaluate that representation; nothing constructs or
//! consumes it yet, hence the module-wide `dead_code` allowance.

// dead_code: experimental, not yet used. non_camel_case_types: the format
// aliases below mirror the `ColorFormat` constant names 1:1 on purpose.
#![allow(dead_code, non_camel_case_types)]

use crate::common::buffer2::Buffer2;

/// A deinterleaved image: `N` channel planes (`1..=4`) of element type `T`, all
/// sharing the same dimensions. One `Buffer2<T>` per channel.
///
/// `N` and `T` live in the type, so e.g. an RGB f32 image is `ImageData<3, f32>`
/// and the channel count is known at compile time (no runtime format tag). The
/// `1..=4` bound is checked per monomorphization via a `const` assert in the
/// constructors.
#[derive(Debug, Clone)]
pub(crate) struct ImageData<const N: usize, T> {
    /// One plane per channel; all share `width × height`.
    pub(crate) channels: [Buffer2<T>; N],
}

impl<const N: usize, T> ImageData<N, T> {
    /// Wrap `N` channel planes. All planes must share the same dimensions.
    pub(crate) fn from_channels(channels: [Buffer2<T>; N]) -> Self {
        const { assert!(N >= 1 && N <= 4, "ImageData supports 1..=4 channels") };
        let w = channels[0].width();
        let h = channels[0].height();
        for plane in &channels {
            assert_eq!(plane.width(), w, "all channel planes must share width");
            assert_eq!(plane.height(), h, "all channel planes must share height");
        }
        Self { channels }
    }

    /// Width in pixels (shared across all planes).
    pub(crate) fn width(&self) -> usize {
        self.channels[0].width()
    }

    /// Height in pixels (shared across all planes).
    pub(crate) fn height(&self) -> usize {
        self.channels[0].height()
    }
}

impl<const N: usize, T: Default + Clone> ImageData<N, T> {
    /// A `width × height` image with all `N` planes zero-filled.
    pub(crate) fn new_zeroed(width: usize, height: usize) -> Self {
        const { assert!(N >= 1 && N <= 4, "ImageData supports 1..=4 channels") };
        Self {
            channels: std::array::from_fn(|_| Buffer2::new_default(width, height)),
        }
    }
}

/// Planar counterparts of every shipping pixel format — the deinterleaved twin
/// of each `ColorFormat` constant (same names, `N`/`T` from the channel count
/// and element type).
pub(crate) type L_U8 = ImageData<1, u8>;
pub(crate) type L_U16 = ImageData<1, u16>;
pub(crate) type L_F32 = ImageData<1, f32>;

pub(crate) type RGB_U8 = ImageData<3, u8>;
pub(crate) type RGB_U16 = ImageData<3, u16>;
pub(crate) type RGB_F32 = ImageData<3, f32>;

pub(crate) type RGBA_U8 = ImageData<4, u8>;
pub(crate) type RGBA_U16 = ImageData<4, u16>;
pub(crate) type RGBA_F32 = ImageData<4, f32>;

/// A planar image of *any* shipping format: one variant per `ColorFormat`,
/// each owning the concrete `ImageData<N, T>`. The type-erased counterpart of
/// the statically-typed `ImageData` — the planar analogue of
/// `image::DynamicImage`, for crossing the runtime-format boundary (decode,
/// node graph) where `N`/`T` aren't known at compile time.
#[derive(Debug, Clone)]
pub(crate) enum AnyImageData {
    L_U8(L_U8),
    L_U16(L_U16),
    L_F32(L_F32),
    RGB_U8(RGB_U8),
    RGB_U16(RGB_U16),
    RGB_F32(RGB_F32),
    RGBA_U8(RGBA_U8),
    RGBA_U16(RGBA_U16),
    RGBA_F32(RGBA_F32),
}

/// Run `$body` against the inner `ImageData` of whichever variant `$self` is,
/// binding it to `$img`. The body must only use methods common to every
/// `ImageData<N, T>` (each arm is type-checked with its own concrete type).
macro_rules! with_inner {
    ($self:expr, $img:ident => $body:expr) => {
        match $self {
            AnyImageData::L_U8($img) => $body,
            AnyImageData::L_U16($img) => $body,
            AnyImageData::L_F32($img) => $body,
            AnyImageData::RGB_U8($img) => $body,
            AnyImageData::RGB_U16($img) => $body,
            AnyImageData::RGB_F32($img) => $body,
            AnyImageData::RGBA_U8($img) => $body,
            AnyImageData::RGBA_U16($img) => $body,
            AnyImageData::RGBA_F32($img) => $body,
        }
    };
}

impl AnyImageData {
    /// Width in pixels, regardless of format.
    pub(crate) fn width(&self) -> usize {
        with_inner!(self, img => img.width())
    }

    /// Height in pixels, regardless of format.
    pub(crate) fn height(&self) -> usize {
        with_inner!(self, img => img.height())
    }

    /// Number of channel planes (1, 3, or 4).
    pub(crate) fn channel_count(&self) -> usize {
        with_inner!(self, img => img.channels.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_zeroed_has_n_zeroed_planes_of_given_size() {
        let img: ImageData<3, f32> = ImageData::new_zeroed(4, 2);
        assert_eq!(img.width(), 4);
        assert_eq!(img.height(), 2);
        assert_eq!(img.channels.len(), 3); // N planes
        for plane in &img.channels {
            assert_eq!(plane.width(), 4);
            assert_eq!(plane.height(), 2);
            assert_eq!(plane.pixels().len(), 8); // 4*2
            assert!(plane.iter().all(|&v| v == 0.0));
        }
    }

    #[test]
    fn from_channels_wraps_planes_and_reports_dims() {
        let r = Buffer2::new(2, 2, vec![1u8, 2, 3, 4]);
        let g = Buffer2::new(2, 2, vec![5u8, 6, 7, 8]);
        let b = Buffer2::new(2, 2, vec![9u8, 10, 11, 12]);
        let img = ImageData::from_channels([r, g, b]);

        assert_eq!(img.width(), 2);
        assert_eq!(img.height(), 2);
        // Planar layout: each channel is contiguous, not interleaved.
        assert_eq!(img.channels[0].pixels(), &[1, 2, 3, 4]);
        assert_eq!(img.channels[1].pixels(), &[5, 6, 7, 8]);
        assert_eq!(img.channels[2].pixels(), &[9, 10, 11, 12]);
    }

    #[test]
    #[should_panic(expected = "all channel planes must share width")]
    fn from_channels_rejects_mismatched_dims() {
        let a = Buffer2::new(2, 2, vec![0u8; 4]);
        let b = Buffer2::new(3, 2, vec![0u8; 6]); // wrong width
        let _ = ImageData::from_channels([a, b]);
    }

    #[test]
    fn channels_are_independently_mutable() {
        let mut img: ImageData<4, u8> = ImageData::new_zeroed(2, 1);
        img.channels[0].pixels_mut()[0] = 255; // R of pixel 0
        img.channels[3].pixels_mut()[1] = 128; // A of pixel 1
        assert_eq!(img.channels[0].pixels(), &[255, 0]);
        assert_eq!(img.channels[1].pixels(), &[0, 0]); // G untouched
        assert_eq!(img.channels[3].pixels(), &[0, 128]);
    }

    #[test]
    fn any_image_data_dispatches_across_variants() {
        let rgb = AnyImageData::RGB_F32(ImageData::new_zeroed(4, 2));
        assert_eq!(rgb.width(), 4);
        assert_eq!(rgb.height(), 2);
        assert_eq!(rgb.channel_count(), 3);

        let gray = AnyImageData::L_U8(ImageData::new_zeroed(5, 3));
        assert_eq!(gray.channel_count(), 1);

        let rgba = AnyImageData::RGBA_U16(ImageData::new_zeroed(1, 1));
        assert_eq!(rgba.channel_count(), 4);

        // The owned `ImageData` is recoverable by matching the variant.
        let AnyImageData::RGB_F32(inner) = rgb else {
            panic!("expected RGB_F32");
        };
        assert_eq!(inner.channels.len(), 3);
    }

    #[test]
    fn type_aliases_match_their_channel_counts() {
        // One representative per channel count + element type.
        let l: L_U8 = ImageData::new_zeroed(1, 1);
        let rgb: RGB_U16 = ImageData::new_zeroed(1, 1);
        let rgba: RGBA_F32 = ImageData::new_zeroed(1, 1);
        assert_eq!(l.channels.len(), 1);
        assert_eq!(rgb.channels.len(), 3);
        assert_eq!(rgba.channels.len(), 4);
        // Element type rides through the alias.
        let _: &[u16] = rgb.channels[0].pixels();
        let _: &[f32] = rgba.channels[0].pixels();
    }
}
