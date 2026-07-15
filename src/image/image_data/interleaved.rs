//! Interleaved image data: [`ImageData<N, T>`] — `Buffer2<[T; N]>`, one `[T; N]`
//! pixel per element (`RGBRGB…`) — plus the runtime-erased [`AnyImageData`] that
//! backs [`Image`](crate::image::Image). Byte-identical to the GPU/file layout,
//! so it casts to `&[u8]` zero-copy. The interleaved twin of the planar
//! [`DeinterleavedImageData`](super::deinterleaved::DeinterleavedImageData).

// dead_code: the type-erased `AnyImageData` isn't wired into a runtime path yet,
// so its `width`/`height`/`color_format` accessors have no production caller
// (`Image` reads format/dims off its own `desc`). non_camel_case_types: the enum
// variants mirror the `ColorFormat` constant names 1:1 on purpose.
#![allow(dead_code, non_camel_case_types)]

use crate::common::buffer2::Buffer2;
use crate::common::color_format::{ChannelCount, ChannelSize, ChannelType, ColorFormat};

/// An interleaved image: `width × height` pixels, each one `[T; N]` element
/// (`RGBRGB…` in memory) in a `Buffer2<[T; N]>`. `N` channels of element type
/// `T`, both carried in the type. The interleaved twin of
/// [`DeinterleavedImageData`](super::deinterleaved::DeinterleavedImageData),
/// byte-identical to a packed [`Image`](crate::image::Image) of the same format
/// — so it casts to `&[u8]` zero-copy. Only the shipping channel counts — `1`
/// (L), `3` (RGB), `4` (RGBA) — are valid.
#[derive(Debug, Clone)]
pub(crate) struct ImageData<const N: usize, T> {
    /// One `[T; N]` pixel per element, `RGBRGB…` interleaved.
    pub(crate) buffer: Buffer2<[T; N]>,
}

impl<const N: usize, T: bytemuck::Pod> ImageData<N, T> {
    /// Wrap an interleaved pixel buffer.
    pub(crate) fn from_buffer(buffer: Buffer2<[T; N]>) -> Self {
        const {
            assert!(
                N == 1 || N == 3 || N == 4,
                "ImageData supports 1, 3, or 4 channels (L/RGB/RGBA)"
            )
        };
        Self { buffer }
    }

    /// A `width × height` interleaved image, zero-filled.
    pub(crate) fn new_zeroed(width: usize, height: usize) -> Self {
        Self::from_buffer(Buffer2::new_filled(width, height, [T::zeroed(); N]))
    }

    /// Build from interleaved bytes (`RGBRGB…`), copying into the typed buffer.
    /// `bytes.len()` must be exactly `width * height * size_of::<[T; N]>()`.
    pub(crate) fn from_bytes(width: usize, height: usize, bytes: &[u8]) -> Self {
        let count = width * height;
        assert_eq!(
            bytes.len(),
            count * std::mem::size_of::<[T; N]>(),
            "byte length must equal width * height * size_of::<[T; N]>()"
        );
        let mut pixels = vec![[T::zeroed(); N]; count];
        bytemuck::cast_slice_mut::<T, u8>(pixels.as_flattened_mut()).copy_from_slice(bytes);
        Self::from_buffer(Buffer2::new(width, height, pixels))
    }

    /// Width in pixels.
    pub(crate) fn width(&self) -> usize {
        self.buffer.width()
    }

    /// Height in pixels.
    pub(crate) fn height(&self) -> usize {
        self.buffer.height()
    }

    /// Interleaved samples as raw bytes — zero-copy view of the pixel buffer.
    pub(crate) fn as_bytes(&self) -> &[u8] {
        bytemuck::cast_slice(self.buffer.pixels().as_flattened())
    }

    /// Interleaved samples as mutable raw bytes — zero-copy; writes hit the buffer.
    pub(crate) fn as_bytes_mut(&mut self) -> &mut [u8] {
        bytemuck::cast_slice_mut(self.buffer.pixels_mut().as_flattened_mut())
    }
}

/// An interleaved image of *any* shipping format: one variant per `ColorFormat`,
/// each owning the concrete [`ImageData<N, T>`]. The runtime-erased counterpart
/// of `ImageData` and the storage behind [`Image`](crate::image::Image) — the
/// interleaved analogue of
/// [`AnyDeinterleavedImageData`](super::deinterleaved::AnyDeinterleavedImageData).
#[derive(Debug, Clone)]
pub(crate) enum AnyImageData {
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
    pub(crate) fn new_zeroed(format: ColorFormat, width: usize, height: usize) -> Self {
        by_format!(format, new_zeroed(width, height))
    }

    /// Build from interleaved bytes for `format` at `width × height` (copies the
    /// bytes into the typed buffer).
    pub(crate) fn from_bytes(
        format: ColorFormat,
        width: usize,
        height: usize,
        bytes: &[u8],
    ) -> Self {
        by_format!(format, from_bytes(width, height, bytes))
    }

    /// The pixel format of this image (derived from the variant).
    pub(crate) fn color_format(&self) -> ColorFormat {
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
    pub(crate) fn width(&self) -> usize {
        with_interleaved!(self, img => img.width())
    }

    /// Height in pixels, regardless of format.
    pub(crate) fn height(&self) -> usize {
        with_interleaved!(self, img => img.height())
    }

    /// Interleaved samples as raw bytes (zero-copy).
    pub(crate) fn bytes(&self) -> &[u8] {
        with_interleaved!(self, img => img.as_bytes())
    }

    /// Interleaved samples as mutable raw bytes (zero-copy).
    pub(crate) fn bytes_mut(&mut self) -> &mut [u8] {
        with_interleaved!(self, img => img.as_bytes_mut())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_zeroed_is_zeroed_and_sized() {
        let img: ImageData<3, f32> = ImageData::new_zeroed(4, 2);
        assert_eq!(img.width(), 4);
        assert_eq!(img.height(), 2);
        assert_eq!(img.buffer.pixels().len(), 8); // 4*2 pixels, one [f32; 3] each
        assert!(img.buffer.pixels().iter().all(|px| *px == [0.0; 3]));
        assert!(img.as_bytes().iter().all(|&b| b == 0));
    }

    #[test]
    fn from_bytes_groups_samples_interleaved_and_round_trips() {
        // RGB_U8, 2x1: two pixels, RGBRGB.
        let bytes = [1u8, 2, 3, 4, 5, 6];
        let img: ImageData<3, u8> = ImageData::from_bytes(2, 1, &bytes);
        assert_eq!(img.width(), 2);
        assert_eq!(img.height(), 1);
        // Interleaved: each `[T; N]` element is one whole pixel.
        assert_eq!(img.buffer.pixels(), &[[1, 2, 3], [4, 5, 6]]);
        // Zero-copy byte view returns exactly the input bytes.
        assert_eq!(img.as_bytes(), &bytes);
    }

    #[test]
    fn as_bytes_mut_writes_through_to_the_buffer() {
        let mut img: ImageData<1, u8> = ImageData::new_zeroed(3, 1);
        img.as_bytes_mut().copy_from_slice(&[7, 8, 9]);
        assert_eq!(img.buffer.pixels(), &[[7], [8], [9]]);
    }

    #[test]
    fn f32_from_bytes_round_trips_values() {
        let pixels = [0.25f32, 0.5, 0.75, 1.0]; // L_F32, 4x1
        let bytes = bytemuck::cast_slice::<f32, u8>(&pixels);
        let img: ImageData<1, f32> = ImageData::from_bytes(4, 1, bytes);
        let back: &[f32] = bytemuck::cast_slice(img.as_bytes());
        assert_eq!(back, &pixels);
    }

    #[test]
    #[should_panic(expected = "byte length must equal")]
    fn from_bytes_rejects_wrong_length() {
        let _: ImageData<3, u8> = ImageData::from_bytes(2, 1, &[1, 2, 3]); // needs 6 bytes
    }

    #[test]
    fn any_new_zeroed_picks_variant_and_reports_format() {
        let img = AnyImageData::new_zeroed(ColorFormat::RGB_F32, 4, 2);
        // color_format() is a 1:1 map from the variant, so this also pins the variant.
        assert_eq!(img.color_format(), ColorFormat::RGB_F32);
        assert_eq!(img.width(), 4);
        assert_eq!(img.height(), 2);
        assert_eq!(img.bytes().len(), 4 * 2 * 3 * 4); // w*h*N*size_of::<f32>
        assert!(img.bytes().iter().all(|&b| b == 0));
    }

    #[test]
    fn any_from_bytes_dispatches_each_format_to_its_variant() {
        // (format, expected total bytes for a 1x1 image).
        let cases = [
            (ColorFormat::L_U8, 1usize),
            (ColorFormat::RGB_U8, 3),
            (ColorFormat::RGBA_U16, 8),
            (ColorFormat::RGB_F32, 12),
        ];
        for (format, byte_len) in cases {
            let bytes = vec![0u8; byte_len];
            let img = AnyImageData::from_bytes(format, 1, 1, &bytes);
            assert_eq!(img.color_format(), format);
            assert_eq!(img.bytes().len(), byte_len);
        }
    }

    #[test]
    fn any_bytes_mut_round_trips_through_format_dispatch() {
        let mut img = AnyImageData::new_zeroed(ColorFormat::RGBA_U8, 1, 1);
        img.bytes_mut().copy_from_slice(&[10, 20, 30, 40]);
        assert_eq!(img.bytes(), &[10, 20, 30, 40]);
    }
}
