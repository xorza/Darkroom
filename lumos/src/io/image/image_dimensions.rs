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
        Self { size, channels }
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
