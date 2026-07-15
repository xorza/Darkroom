//! Planar image data: [`DeinterleavedImageData<N, T>`] — one `Buffer2<T>` per
//! channel (`RRR…GGG…BBB…`) — plus the runtime-erased [`AnyDeinterleavedImageData`].
//! The planar twin of the interleaved
//! [`ImageData`](super::interleaved::ImageData); nicer for per-channel CPU work,
//! and the storage behind lumos's `AstroImage` pixel data.

// dead_code: the erased `AnyDeinterleavedImageData` isn't wired into the pipeline
// yet (it lands with the interleave/deinterleave conversions); lumos uses the
// typed `DeinterleavedImageData` directly. non_camel_case_types: the enum
// variants mirror the `ColorFormat` constant names 1:1 on purpose.
#![allow(dead_code, non_camel_case_types)]

use crate::common::buffer2::Buffer2;

/// A deinterleaved image: `N` channel planes (`1`, `3`, or `4`) of element type
/// `T`, all sharing the same dimensions. One `Buffer2<T>` per channel.
///
/// `N` and `T` live in the type, so e.g. an RGB f32 image is
/// `DeinterleavedImageData<3, f32>` and the channel count is known at compile
/// time (no runtime format tag). Only the shipping channel counts — `1` (L),
/// `3` (RGB), `4` (RGBA) — are valid; that's checked per monomorphization via a
/// `const` assert in the constructors.
#[derive(Debug, Clone)]
pub struct DeinterleavedImageData<const N: usize, T> {
    /// One plane per channel; all share `width × height`.
    pub channels: [Buffer2<T>; N],
}

impl<const N: usize, T> DeinterleavedImageData<N, T> {
    /// Wrap `N` channel planes. All planes must share the same dimensions.
    pub fn from_channels(channels: [Buffer2<T>; N]) -> Self {
        const {
            assert!(
                N == 1 || N == 3 || N == 4,
                "DeinterleavedImageData supports 1, 3, or 4 channels (L/RGB/RGBA)"
            )
        };
        let w = channels[0].width();
        let h = channels[0].height();
        for plane in &channels {
            assert_eq!(plane.width(), w, "all channel planes must share width");
            assert_eq!(plane.height(), h, "all channel planes must share height");
        }
        Self { channels }
    }

    /// Width in pixels (shared across all planes).
    pub fn width(&self) -> usize {
        self.channels[0].width()
    }

    /// Height in pixels (shared across all planes).
    pub fn height(&self) -> usize {
        self.channels[0].height()
    }
}

impl<const N: usize, T: Default + Clone> DeinterleavedImageData<N, T> {
    /// A `width × height` image with all `N` planes zero-filled.
    pub fn new_zeroed(width: usize, height: usize) -> Self {
        // Channel-count invariant + (trivially-satisfied) dim check live in `from_channels`.
        Self::from_channels(std::array::from_fn(|_| Buffer2::new_default(width, height)))
    }
}

/// A planar image of *any* shipping format: one variant per `ColorFormat`, each
/// owning the concrete `DeinterleavedImageData<N, T>`. The type-erased
/// counterpart of the statically-typed `DeinterleavedImageData` — the planar
/// analogue of `image::DynamicImage`, for crossing the runtime-format boundary
/// (decode, node graph) where `N`/`T` aren't known at compile time.
#[derive(Debug, Clone)]
pub(crate) enum AnyDeinterleavedImageData {
    L_U8(DeinterleavedImageData<1, u8>),
    L_U16(DeinterleavedImageData<1, u16>),
    L_F32(DeinterleavedImageData<1, f32>),
    RGB_U8(DeinterleavedImageData<3, u8>),
    RGB_U16(DeinterleavedImageData<3, u16>),
    RGB_F32(DeinterleavedImageData<3, f32>),
    RGBA_U8(DeinterleavedImageData<4, u8>),
    RGBA_U16(DeinterleavedImageData<4, u16>),
    RGBA_F32(DeinterleavedImageData<4, f32>),
}

/// Run `$body` against the inner `DeinterleavedImageData` of whichever variant
/// `$self` is, binding it to `$img`. The body must only use methods common to
/// every `DeinterleavedImageData<N, T>` (each arm is type-checked with its own
/// concrete type).
macro_rules! with_deinterleaved {
    ($self:expr, $img:ident => $body:expr) => {
        match $self {
            AnyDeinterleavedImageData::L_U8($img) => $body,
            AnyDeinterleavedImageData::L_U16($img) => $body,
            AnyDeinterleavedImageData::L_F32($img) => $body,
            AnyDeinterleavedImageData::RGB_U8($img) => $body,
            AnyDeinterleavedImageData::RGB_U16($img) => $body,
            AnyDeinterleavedImageData::RGB_F32($img) => $body,
            AnyDeinterleavedImageData::RGBA_U8($img) => $body,
            AnyDeinterleavedImageData::RGBA_U16($img) => $body,
            AnyDeinterleavedImageData::RGBA_F32($img) => $body,
        }
    };
}

impl AnyDeinterleavedImageData {
    /// Width in pixels, regardless of format.
    pub(crate) fn width(&self) -> usize {
        with_deinterleaved!(self, img => img.width())
    }

    /// Height in pixels, regardless of format.
    pub(crate) fn height(&self) -> usize {
        with_deinterleaved!(self, img => img.height())
    }

    /// Number of channel planes (1, 3, or 4).
    pub(crate) fn channel_count(&self) -> usize {
        with_deinterleaved!(self, img => img.channels.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_zeroed_has_n_zeroed_planes_of_given_size() {
        let img: DeinterleavedImageData<3, f32> = DeinterleavedImageData::new_zeroed(4, 2);
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
        let img = DeinterleavedImageData::from_channels([r, g, b]);

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
        let r = Buffer2::new(2, 2, vec![0u8; 4]);
        let g = Buffer2::new(3, 2, vec![0u8; 6]); // wrong width
        let b = Buffer2::new(2, 2, vec![0u8; 4]);
        let _ = DeinterleavedImageData::from_channels([r, g, b]);
    }

    #[test]
    fn channels_are_independently_mutable() {
        let mut img: DeinterleavedImageData<4, u8> = DeinterleavedImageData::new_zeroed(2, 1);
        img.channels[0].pixels_mut()[0] = 255; // R of pixel 0
        img.channels[3].pixels_mut()[1] = 128; // A of pixel 1
        assert_eq!(img.channels[0].pixels(), &[255, 0]);
        assert_eq!(img.channels[1].pixels(), &[0, 0]); // G untouched
        assert_eq!(img.channels[3].pixels(), &[0, 128]);
    }

    #[test]
    fn any_image_data_dispatches_across_variants() {
        let rgb = AnyDeinterleavedImageData::RGB_F32(DeinterleavedImageData::new_zeroed(4, 2));
        assert_eq!(rgb.width(), 4);
        assert_eq!(rgb.height(), 2);
        assert_eq!(rgb.channel_count(), 3);

        let gray = AnyDeinterleavedImageData::L_U8(DeinterleavedImageData::new_zeroed(5, 3));
        assert_eq!(gray.channel_count(), 1);

        // Element type rides through the variant.
        let rgba = AnyDeinterleavedImageData::RGBA_U16(DeinterleavedImageData::new_zeroed(1, 1));
        assert_eq!(rgba.channel_count(), 4);

        // The owned `DeinterleavedImageData` is recoverable by matching the variant.
        let AnyDeinterleavedImageData::RGB_F32(inner) = rgb else {
            panic!("expected RGB_F32");
        };
        assert_eq!(inner.channels.len(), 3);
        let _: &[f32] = inner.channels[0].pixels();
    }
}
