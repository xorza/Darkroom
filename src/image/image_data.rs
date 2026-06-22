//! The two image-data layouts — a mutually-convertible pair, both with `N`/`T`
//! in the type and a runtime-erased enum over the 9 shipping formats:
//!
//! - [`ImageData<N, T>`] — **interleaved**: `Buffer2<[T; N]>`, one `[T; N]` pixel
//!   per element (`RGBRGB…`). Byte-identical to the GPU/file layout, so it casts
//!   to `&[u8]` zero-copy; it backs [`Image`](super::Image) (via [`AnyImageData`]).
//! - [`DeinterleavedImageData<N, T>`] — **planar**: one `Buffer2<T>` per channel
//!   (`RRR…GGG…BBB…`). Nicer for per-channel CPU work; it backs lumos's
//!   `AstroImage` pixel data.
//!
//! The erased forms ([`AnyImageData`] / [`AnyDeinterleavedImageData`]) convert by
//! transpose (interleave / deinterleave).

// non_camel_case_types: the format aliases/variants mirror the `ColorFormat`
// constant names 1:1 on purpose. dead_code: the deinterleaved erased form +
// aliases aren't all wired up yet.
#![allow(dead_code, non_camel_case_types)]

use crate::common::buffer2::Buffer2;
use crate::common::color_format::{ChannelCount, ChannelSize, ChannelType, ColorFormat};

/// A deinterleaved image: `N` channel planes (`1`, `3`, or `4`) of element type
/// `T`, all sharing the same dimensions. One `Buffer2<T>` per channel.
///
/// `N` and `T` live in the type, so e.g. an RGB f32 image is `DeinterleavedImageData<3, f32>`
/// and the channel count is known at compile time (no runtime format tag). Only
/// the shipping channel counts — `1` (L), `3` (RGB), `4` (RGBA) — are valid;
/// that's checked per monomorphization via a `const` assert in the constructors.
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
        const {
            assert!(
                N == 1 || N == 3 || N == 4,
                "DeinterleavedImageData supports 1, 3, or 4 channels (L/RGB/RGBA)"
            )
        };
        Self {
            channels: std::array::from_fn(|_| Buffer2::new_default(width, height)),
        }
    }
}

/// Planar counterparts of every shipping pixel format — the deinterleaved twin
/// of each `ColorFormat` constant (same names, `N`/`T` from the channel count
/// and element type).
pub(crate) type L_U8 = DeinterleavedImageData<1, u8>;
pub(crate) type L_U16 = DeinterleavedImageData<1, u16>;
pub(crate) type L_F32 = DeinterleavedImageData<1, f32>;

pub(crate) type RGB_U8 = DeinterleavedImageData<3, u8>;
pub(crate) type RGB_U16 = DeinterleavedImageData<3, u16>;
pub(crate) type RGB_F32 = DeinterleavedImageData<3, f32>;

pub(crate) type RGBA_U8 = DeinterleavedImageData<4, u8>;
pub(crate) type RGBA_U16 = DeinterleavedImageData<4, u16>;
pub(crate) type RGBA_F32 = DeinterleavedImageData<4, f32>;

/// A planar image of *any* shipping format: one variant per `ColorFormat`,
/// each owning the concrete `DeinterleavedImageData<N, T>`. The type-erased counterpart of
/// the statically-typed `DeinterleavedImageData` — the planar analogue of
/// `image::DynamicImage`, for crossing the runtime-format boundary (decode,
/// node graph) where `N`/`T` aren't known at compile time.
#[derive(Debug, Clone)]
pub(crate) enum AnyDeinterleavedImageData {
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

/// Run `$body` against the inner `DeinterleavedImageData` of whichever variant `$self` is,
/// binding it to `$img`. The body must only use methods common to every
/// `DeinterleavedImageData<N, T>` (each arm is type-checked with its own concrete type).
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
    pub fn width(&self) -> usize {
        with_deinterleaved!(self, img => img.width())
    }

    /// Height in pixels, regardless of format.
    pub fn height(&self) -> usize {
        with_deinterleaved!(self, img => img.height())
    }

    /// Number of channel planes (1, 3, or 4).
    pub(crate) fn channel_count(&self) -> usize {
        with_deinterleaved!(self, img => img.channels.len())
    }
}

/// An interleaved image: `width × height` pixels, each one `[T; N]` element
/// (`RGBRGB…` in memory) in a `Buffer2<[T; N]>`. `N` channels of element type
/// `T`, both carried in the type. The interleaved twin of
/// [`DeinterleavedImageData`], byte-identical to a packed [`Image`](super::Image)
/// of the same format — so it casts to `&[u8]` zero-copy. Only the shipping
/// channel counts — `1` (L), `3` (RGB), `4` (RGBA) — are valid.
#[derive(Debug, Clone)]
pub struct ImageData<const N: usize, T> {
    /// One `[T; N]` pixel per element, `RGBRGB…` interleaved.
    pub buffer: Buffer2<[T; N]>,
}

impl<const N: usize, T: bytemuck::Pod> ImageData<N, T>
where
    [T; N]: bytemuck::Pod,
{
    /// Wrap an interleaved pixel buffer.
    pub fn from_buffer(buffer: Buffer2<[T; N]>) -> Self {
        const {
            assert!(
                N == 1 || N == 3 || N == 4,
                "ImageData supports 1, 3, or 4 channels (L/RGB/RGBA)"
            )
        };
        Self { buffer }
    }

    /// A `width × height` interleaved image, zero-filled.
    pub fn new_zeroed(width: usize, height: usize) -> Self {
        Self::from_buffer(Buffer2::new_filled(width, height, [T::zeroed(); N]))
    }

    /// Build from interleaved bytes (`RGBRGB…`), copying into the typed buffer.
    /// `bytes.len()` must be exactly `width * height * size_of::<[T; N]>()`.
    pub fn from_bytes(width: usize, height: usize, bytes: &[u8]) -> Self {
        let count = width * height;
        assert_eq!(
            bytes.len(),
            count * std::mem::size_of::<[T; N]>(),
            "byte length must equal width * height * size_of::<[T; N]>()"
        );
        let mut pixels = vec![[T::zeroed(); N]; count];
        bytemuck::cast_slice_mut::<[T; N], u8>(&mut pixels).copy_from_slice(bytes);
        Self::from_buffer(Buffer2::new(width, height, pixels))
    }

    /// Width in pixels.
    pub fn width(&self) -> usize {
        self.buffer.width()
    }

    /// Height in pixels.
    pub fn height(&self) -> usize {
        self.buffer.height()
    }

    /// Interleaved samples as raw bytes — zero-copy view of the pixel buffer.
    pub fn as_bytes(&self) -> &[u8] {
        bytemuck::cast_slice(self.buffer.pixels())
    }

    /// Interleaved samples as mutable raw bytes — zero-copy; writes hit the buffer.
    pub fn as_bytes_mut(&mut self) -> &mut [u8] {
        bytemuck::cast_slice_mut(self.buffer.pixels_mut())
    }
}

/// An interleaved image of *any* shipping format: one variant per `ColorFormat`,
/// each owning the concrete [`ImageData<N, T>`]. The runtime-erased counterpart
/// of `ImageData` and the storage behind [`Image`](super::Image) — the
/// interleaved analogue of [`AnyDeinterleavedImageData`].
#[derive(Debug, Clone)]
pub enum AnyImageData {
    L_U8(ImageData<1, u8>),
    L_U16(ImageData<1, u16>),
    L_F32(ImageData<1, f32>),
    RGB_U8(ImageData<3, u8>),
    RGB_U16(ImageData<3, u16>),
    RGB_F32(ImageData<3, f32>),
    RGBA_U8(ImageData<4, u8>),
    RGBA_U16(ImageData<4, u16>),
    RGBA_F32(ImageData<4, f32>),
}

/// Run `$body` against the inner `ImageData` of whichever variant `$self` is,
/// binding it to `$img`. The body must only use methods common to every
/// `ImageData<N, T>` (each arm is type-checked with its own concrete type).
macro_rules! with_interleaved {
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

/// Build an `AnyImageData` by mapping `$format` to its `(N, T)` variant and
/// invoking `ImageData::$ctor($args…)` for that concrete type. Centralizes the
/// 9-way format dispatch so each constructor doesn't repeat it.
macro_rules! by_format {
    ($format:expr, $ctor:ident ( $($arg:expr),* )) => {{
        let f = $format;
        match (f.channel_count, f.channel_size, f.channel_type) {
            (ChannelCount::L, ChannelSize::_8bit, ChannelType::UInt) => AnyImageData::L_U8(ImageData::$ctor($($arg),*)),
            (ChannelCount::L, ChannelSize::_16bit, ChannelType::UInt) => AnyImageData::L_U16(ImageData::$ctor($($arg),*)),
            (ChannelCount::L, ChannelSize::_32bit, ChannelType::Float) => AnyImageData::L_F32(ImageData::$ctor($($arg),*)),
            (ChannelCount::Rgb, ChannelSize::_8bit, ChannelType::UInt) => AnyImageData::RGB_U8(ImageData::$ctor($($arg),*)),
            (ChannelCount::Rgb, ChannelSize::_16bit, ChannelType::UInt) => AnyImageData::RGB_U16(ImageData::$ctor($($arg),*)),
            (ChannelCount::Rgb, ChannelSize::_32bit, ChannelType::Float) => AnyImageData::RGB_F32(ImageData::$ctor($($arg),*)),
            (ChannelCount::Rgba, ChannelSize::_8bit, ChannelType::UInt) => AnyImageData::RGBA_U8(ImageData::$ctor($($arg),*)),
            (ChannelCount::Rgba, ChannelSize::_16bit, ChannelType::UInt) => AnyImageData::RGBA_U16(ImageData::$ctor($($arg),*)),
            (ChannelCount::Rgba, ChannelSize::_32bit, ChannelType::Float) => AnyImageData::RGBA_F32(ImageData::$ctor($($arg),*)),
            _ => panic!("AnyImageData: unsupported color format {f:?}"),
        }
    }};
}

impl AnyImageData {
    /// A zeroed interleaved image for `format` at `width × height`.
    pub fn new_zeroed(format: ColorFormat, width: usize, height: usize) -> Self {
        by_format!(format, new_zeroed(width, height))
    }

    /// Build from interleaved bytes for `format` at `width × height` (copies the
    /// bytes into the typed buffer).
    pub fn from_bytes(format: ColorFormat, width: usize, height: usize, bytes: &[u8]) -> Self {
        by_format!(format, from_bytes(width, height, bytes))
    }

    /// The pixel format of this image (derived from the variant).
    pub fn color_format(&self) -> ColorFormat {
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

    /// Width in pixels, regardless of format.
    pub fn width(&self) -> usize {
        with_interleaved!(self, img => img.width())
    }

    /// Height in pixels, regardless of format.
    pub fn height(&self) -> usize {
        with_interleaved!(self, img => img.height())
    }

    /// Interleaved samples as raw bytes (zero-copy).
    pub fn bytes(&self) -> &[u8] {
        with_interleaved!(self, img => img.as_bytes())
    }

    /// Interleaved samples as mutable raw bytes (zero-copy).
    pub fn bytes_mut(&mut self) -> &mut [u8] {
        with_interleaved!(self, img => img.as_bytes_mut())
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

        let rgba = AnyDeinterleavedImageData::RGBA_U16(DeinterleavedImageData::new_zeroed(1, 1));
        assert_eq!(rgba.channel_count(), 4);

        // The owned `DeinterleavedImageData` is recoverable by matching the variant.
        let AnyDeinterleavedImageData::RGB_F32(inner) = rgb else {
            panic!("expected RGB_F32");
        };
        assert_eq!(inner.channels.len(), 3);
    }

    #[test]
    fn type_aliases_match_their_channel_counts() {
        // One representative per channel count + element type.
        let l: L_U8 = DeinterleavedImageData::new_zeroed(1, 1);
        let rgb: RGB_U16 = DeinterleavedImageData::new_zeroed(1, 1);
        let rgba: RGBA_F32 = DeinterleavedImageData::new_zeroed(1, 1);
        assert_eq!(l.channels.len(), 1);
        assert_eq!(rgb.channels.len(), 3);
        assert_eq!(rgba.channels.len(), 4);
        // Element type rides through the alias.
        let _: &[u16] = rgb.channels[0].pixels();
        let _: &[f32] = rgba.channels[0].pixels();
    }
}
