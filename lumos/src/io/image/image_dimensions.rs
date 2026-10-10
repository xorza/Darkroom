//! Pixel extent plus channel count, validated once at construction.

use crate::memory;
use std::fmt;

use crate::io::image::flat_gain::GainGrid;
use crate::math::size2us::Size2us;

/// Image dimensions: pixel size and number of channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ImageDimensions {
    size: Size2us,
    channels: usize,
}

impl ImageDimensions {
    /// The longest side an image may have, 2³⁰ px — some 7000 times a large sensor's.
    ///
    /// Pixel addressing in the resampler and the detector runs in `i32` lanes, and a side this far
    /// under `i32::MAX` leaves every coordinate, and a kernel's reach past either edge, inside that
    /// range. The decoders refuse a larger image before it gets here.
    pub const MAX_SIDE: usize = 1 << 30;

    pub fn new(size: impl Into<Size2us>, channels: usize) -> Self {
        let size = size.into();
        Self::validate(size, channels);
        Self { size, channels }
    }

    /// Panic unless `size` and `channels` describe a representable image.
    ///
    /// The same contract [`Self::new`] enforces, split out for callers that carry the extent in
    /// some other shape and want the check without the value — building an `ImageDimensions` only
    /// to drop it reads like a mistake.
    pub(crate) fn validate(size: impl Into<Size2us>, channels: usize) {
        let size = size.into();
        assert!(size.width > 0, "Width must be positive");
        assert!(size.height > 0, "Height must be positive");
        assert!(
            size.width <= Self::MAX_SIDE && size.height <= Self::MAX_SIDE,
            "{}x{} has a side past MAX_SIDE",
            size.width,
            size.height
        );
        assert!(
            channels == 1 || channels == 3,
            "Only 1 (grayscale) or 3 (RGB) channels supported, got {channels}"
        );
        size.pixel_count()
            .checked_mul(channels)
            .expect("Image sample count must fit in usize");
    }

    /// Pixel extent, without the channel count.
    pub const fn size(&self) -> Size2us {
        self.size
    }

    pub const fn width(&self) -> usize {
        self.size.width
    }

    pub const fn height(&self) -> usize {
        self.size.height
    }

    pub const fn channels(&self) -> usize {
        self.channels
    }

    /// Total number of f32 samples: `width * height * channels`.
    /// For a 100x100 RGB image, returns 30000.
    pub const fn sample_count(&self) -> usize {
        self.pixel_count()
            .checked_mul(self.channels)
            .expect("ImageDimensions validates sample count during construction")
    }

    /// Number of pixels: `width * height`.
    /// For a 100x100 RGB image, returns 10000.
    pub const fn pixel_count(&self) -> usize {
        self.size.pixel_count()
    }

    pub const fn is_grayscale(&self) -> bool {
        self.channels == 1
    }

    pub const fn is_rgb(&self) -> bool {
        self.channels == 3
    }

    /// Bytes the image's pixels occupy, planar f32.
    pub(crate) const fn frame_bytes(&self) -> usize {
        self.sample_count() * size_of::<f32>()
    }

    /// Bytes the quality planes add to a frame that carries them.
    ///
    /// One plane per pixel rather than per sample: coverage and confidence are channel-independent,
    /// so an RGB frame pays for two planes here, not six.
    pub(crate) const fn quality_plane_bytes(&self) -> usize {
        memory::FRAME_QUALITY_PLANES * self.pixel_count() * size_of::<f32>()
    }

    /// Bytes a frame's flag plane adds: one per pixel, whatever its channel count. Charged to every
    /// frame, since a decoder that flags saturation, and a calibration that flags its repairs, give
    /// one to most frames.
    pub(crate) const fn flag_plane_bytes(&self) -> usize {
        self.pixel_count()
    }

    /// Bytes a warped frame's flat gain grid adds: a node grid per channel, about a sixteenth of
    /// a plane each. Charged to every warped frame, since lights are divided by a flat as a rule.
    pub(crate) const fn flat_gain_bytes(&self) -> usize {
        let grid = GainGrid::of(self.size());
        self.channels() * grid.columns * grid.rows * size_of::<f32>()
    }
}

/// `width×height×channels` — the form error messages quote geometry in, where the derived `Debug`
/// would spell out two nested structs.
impl fmt::Display for ImageDimensions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}x{}x{}", self.width(), self.height(), self.channels)
    }
}

#[cfg(test)]
mod tests {
    use crate::internals::panic_message;
    use crate::io::image::image_dimensions::ImageDimensions;
    use std::panic::catch_unwind;

    #[test]
    fn validate_accepts_exactly_what_new_accepts() {
        for channels in [1, 3] {
            ImageDimensions::validate((4, 3), channels);
            let dimensions = ImageDimensions::new((4, 3), channels);
            assert_eq!(dimensions.size(), (4, 3).into());
            assert_eq!(dimensions.channels(), channels);
            assert_eq!(dimensions.pixel_count(), 12);
            assert_eq!(dimensions.sample_count(), 12 * channels);
            assert_eq!(dimensions.is_grayscale(), channels == 1);
            assert_eq!(dimensions.is_rgb(), channels == 3);
        }

        // A flat gain grid has a node every 4 pixels from the first, the last at or past the last
        // pixel, 4 bytes each per channel: 9×9 takes 3×3 nodes, 3·9·4 = 108 bytes in colour; 13×5
        // takes 4×2, 32 bytes in mono.
        assert_eq!(ImageDimensions::new((9, 9), 3).flat_gain_bytes(), 108);
        assert_eq!(ImageDimensions::new((13, 5), 1).flat_gain_bytes(), 32);

        for (width, height, channels, expected) in [
            (0, 3, 1, "Width must be positive"),
            (4, 0, 1, "Height must be positive"),
            (4, 3, 0, "channels supported, got 0"),
            (4, 3, 2, "channels supported, got 2"),
            (4, 3, 4, "channels supported, got 4"),
            (
                ImageDimensions::MAX_SIDE + 1,
                1,
                1,
                "has a side past MAX_SIDE",
            ),
            (
                1,
                ImageDimensions::MAX_SIDE + 1,
                1,
                "has a side past MAX_SIDE",
            ),
        ] {
            let panic = catch_unwind(|| ImageDimensions::validate((width, height), channels))
                .expect_err("must be rejected");
            let message = panic_message(&*panic);
            assert!(
                message.contains(expected),
                "{width}x{height}x{channels} reported {message:?}, wanted {expected:?}"
            );
            // `new` cannot be more permissive than the check it delegates to.
            assert!(
                catch_unwind(|| ImageDimensions::new((width, height), channels)).is_err(),
                "new accepted {width}x{height}x{channels}"
            );
        }
    }
}
