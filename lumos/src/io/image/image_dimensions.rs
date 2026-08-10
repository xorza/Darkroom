//! Pixel extent plus channel count, validated once at construction.

use crate::math::size2us::Size2us;

/// Image dimensions: pixel size and number of channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ImageDimensions {
    size: Size2us,
    channels: usize,
}

impl ImageDimensions {
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
            channels == 1 || channels == 3,
            "Only 1 (grayscale) or 3 (RGB) channels supported, got {}",
            channels
        );
        size.pixel_count()
            .checked_mul(channels)
            .expect("Image sample count must fit in usize");
    }

    /// Pixel extent, without the channel count.
    pub fn size(&self) -> Size2us {
        self.size
    }

    pub fn width(&self) -> usize {
        self.size.width
    }

    pub fn height(&self) -> usize {
        self.size.height
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Total number of f32 samples: `width * height * channels`.
    /// For a 100x100 RGB image, returns 30000.
    pub fn sample_count(&self) -> usize {
        self.pixel_count()
            .checked_mul(self.channels)
            .expect("ImageDimensions validates sample count during construction")
    }

    /// Number of pixels: `width * height`.
    /// For a 100x100 RGB image, returns 10000.
    pub fn pixel_count(&self) -> usize {
        self.size.pixel_count()
    }

    pub fn is_grayscale(&self) -> bool {
        self.channels == 1
    }

    pub fn is_rgb(&self) -> bool {
        self.channels == 3
    }
}

#[cfg(test)]
mod tests {
    use crate::io::image::image_dimensions::ImageDimensions;
    use std::panic::catch_unwind;

    #[test]
    fn validate_accepts_exactly_what_new_accepts() {
        for channels in [1, 3] {
            ImageDimensions::validate((4, 3), channels);
            let dimensions = ImageDimensions::new((4, 3), channels);
            assert_eq!(dimensions.sample_count(), 12 * channels);
        }

        for (width, height, channels, expected) in [
            (0, 3, 1, "Width must be positive"),
            (4, 0, 1, "Height must be positive"),
            (4, 3, 0, "channels supported, got 0"),
            (4, 3, 2, "channels supported, got 2"),
            (4, 3, 4, "channels supported, got 4"),
            // 3 channels × (usize::MAX / 2) pixels overflows the sample count.
            (usize::MAX / 2, 1, 3, "Image sample count must fit in usize"),
        ] {
            let panic = catch_unwind(|| ImageDimensions::validate((width, height), channels))
                .expect_err("must be rejected");
            let message = panic
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| panic.downcast_ref::<&str>().map(|s| (*s).to_string()))
                .unwrap_or_default();
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
