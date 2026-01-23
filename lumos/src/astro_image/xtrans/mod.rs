//! X-Trans CFA demosaicing module.
//!
//! Provides demosaicing for Fujifilm X-Trans sensors which use a 6x6 CFA pattern
//! instead of the standard 2x2 Bayer pattern.
//!
//! The X-Trans pattern has ~55% green, ~22.5% red, and ~22.5% blue pixels arranged
//! so that every row and column contains all three colors.

mod scalar;

pub use scalar::demosaic_xtrans_bilinear;

/// X-Trans 6x6 color filter array pattern.
/// Values: 0=Red, 1=Green, 2=Blue
#[derive(Debug, Clone)]
pub struct XTransPattern {
    /// 6x6 pattern array indexed by [row % 6][col % 6]
    pub pattern: [[u8; 6]; 6],
}

impl XTransPattern {
    /// Create a new X-Trans pattern from a 6x6 array.
    ///
    /// # Panics
    /// Panics if any value in the pattern is not 0, 1, or 2.
    pub fn new(pattern: [[u8; 6]; 6]) -> Self {
        for row in &pattern {
            for &val in row {
                assert!(
                    val <= 2,
                    "Invalid X-Trans pattern value: {} (must be 0, 1, or 2)",
                    val
                );
            }
        }
        Self { pattern }
    }

    /// Get the color at position (row, col).
    /// Returns: 0=Red, 1=Green, 2=Blue
    #[inline(always)]
    pub fn color_at(&self, row: usize, col: usize) -> u8 {
        self.pattern[row % 6][col % 6]
    }
}

/// Raw X-Trans image data with metadata needed for demosaicing.
#[derive(Debug)]
pub struct XTransImage<'a> {
    /// Raw pixel data (normalized to 0.0-1.0)
    pub data: &'a [f32],
    /// Width of the raw data buffer
    pub raw_width: usize,
    /// Height of the raw data buffer
    pub raw_height: usize,
    /// Width of the active/output image area
    pub width: usize,
    /// Height of the active/output image area
    pub height: usize,
    /// Top margin (offset from raw to active area)
    pub top_margin: usize,
    /// Left margin (offset from raw to active area)
    pub left_margin: usize,
    /// X-Trans CFA pattern
    pub pattern: XTransPattern,
}

impl<'a> XTransImage<'a> {
    /// Create an XTransImage with margins.
    ///
    /// # Panics
    /// Panics if:
    /// - `data.len() != raw_width * raw_height`
    /// - `top_margin + height > raw_height`
    /// - `left_margin + width > raw_width`
    /// - `width == 0` or `height == 0`
    #[allow(clippy::too_many_arguments)]
    pub fn with_margins(
        data: &'a [f32],
        raw_width: usize,
        raw_height: usize,
        width: usize,
        height: usize,
        top_margin: usize,
        left_margin: usize,
        pattern: XTransPattern,
    ) -> Self {
        assert!(
            width > 0 && height > 0,
            "Output dimensions must be non-zero: {}x{}",
            width,
            height
        );
        assert!(
            raw_width > 0 && raw_height > 0,
            "Raw dimensions must be non-zero: {}x{}",
            raw_width,
            raw_height
        );
        assert!(
            data.len() == raw_width * raw_height,
            "Data length {} doesn't match raw dimensions {}x{}={}",
            data.len(),
            raw_width,
            raw_height,
            raw_width * raw_height
        );
        assert!(
            top_margin + height <= raw_height,
            "Top margin {} + height {} exceeds raw height {}",
            top_margin,
            height,
            raw_height
        );
        assert!(
            left_margin + width <= raw_width,
            "Left margin {} + width {} exceeds raw width {}",
            left_margin,
            width,
            raw_width
        );

        Self {
            data,
            raw_width,
            raw_height,
            width,
            height,
            top_margin,
            left_margin,
            pattern,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Standard X-Trans pattern (one common variant)
    fn test_pattern() -> XTransPattern {
        XTransPattern::new([
            [1, 0, 1, 1, 2, 1], // G R G G B G
            [2, 1, 2, 0, 1, 0], // B G B R G R
            [1, 2, 1, 1, 0, 1], // G B G G R G
            [1, 2, 1, 1, 0, 1], // G B G G R G
            [0, 1, 0, 2, 1, 2], // R G R B G B
            [1, 0, 1, 1, 2, 1], // G R G G B G
        ])
    }

    #[test]
    fn test_xtrans_pattern_color_at() {
        let pattern = test_pattern();
        // Check corners
        assert_eq!(pattern.color_at(0, 0), 1); // G
        assert_eq!(pattern.color_at(0, 1), 0); // R
        assert_eq!(pattern.color_at(0, 4), 2); // B
        assert_eq!(pattern.color_at(1, 0), 2); // B
        // Check wrapping
        assert_eq!(pattern.color_at(6, 0), pattern.color_at(0, 0));
        assert_eq!(pattern.color_at(0, 6), pattern.color_at(0, 0));
        assert_eq!(pattern.color_at(12, 12), pattern.color_at(0, 0));
    }

    #[test]
    #[should_panic(expected = "Invalid X-Trans pattern value")]
    fn test_xtrans_pattern_invalid_value() {
        XTransPattern::new([
            [1, 0, 1, 1, 2, 1],
            [2, 1, 3, 0, 1, 0], // 3 is invalid
            [1, 2, 1, 1, 0, 1],
            [1, 2, 1, 1, 0, 1],
            [0, 1, 0, 2, 1, 2],
            [1, 0, 1, 1, 2, 1],
        ]);
    }

    #[test]
    fn test_xtrans_image_valid() {
        let data = vec![0.5f32; 36];
        let pattern = test_pattern();
        let img = XTransImage::with_margins(&data, 6, 6, 4, 4, 1, 1, pattern);
        assert_eq!(img.raw_width, 6);
        assert_eq!(img.raw_height, 6);
        assert_eq!(img.width, 4);
        assert_eq!(img.height, 4);
    }

    #[test]
    #[should_panic(expected = "Output dimensions must be non-zero")]
    fn test_xtrans_image_zero_width() {
        let data = vec![0.5f32; 36];
        let pattern = test_pattern();
        XTransImage::with_margins(&data, 6, 6, 0, 4, 0, 0, pattern);
    }

    #[test]
    #[should_panic(expected = "Data length")]
    fn test_xtrans_image_wrong_data_length() {
        let data = vec![0.5f32; 30]; // Should be 36
        let pattern = test_pattern();
        XTransImage::with_margins(&data, 6, 6, 6, 6, 0, 0, pattern);
    }
}
