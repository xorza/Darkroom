//! X-Trans CFA demosaicing module.
//!
//! Provides demosaicing for Fujifilm X-Trans sensors which use a 6x6 CFA pattern
//! instead of the standard 2x2 Bayer pattern.
//!
//! The X-Trans pattern has ~55% green, ~22.5% red, and ~22.5% blue pixels arranged
//! so that every row and column contains all three colors.
//!
//! Uses the Markesteijn 1-pass algorithm: directional interpolation in 4 directions
//! with homogeneity-based selection for high-quality output.

mod hex_lookup;
mod markesteijn;
mod markesteijn_steps;

use std::time::Instant;

pub use markesteijn::demosaic_xtrans_markesteijn;

/// Process X-Trans sensor data and demosaic to RGB.
///
/// Takes raw u16 sensor data and normalization parameters. Normalization happens
/// on-the-fly during demosaicing, avoiding a separate P×4 byte f32 buffer.
///
/// # Returns
/// Tuple of (RGB pixels, number of channels which is always 3)
#[allow(clippy::too_many_arguments)]
pub fn process_xtrans(
    raw_data: &[u16],
    raw_width: usize,
    raw_height: usize,
    width: usize,
    height: usize,
    top_margin: usize,
    left_margin: usize,
    xtrans_pattern: [[u8; 6]; 6],
    black: f32,
    inv_range: f32,
) -> Vec<f32> {
    let pattern = XTransPattern::new(xtrans_pattern);

    let xtrans = XTransImage::with_margins(
        raw_data,
        raw_width,
        raw_height,
        width,
        height,
        top_margin,
        left_margin,
        pattern,
        black,
        inv_range,
    );

    let demosaic_start = Instant::now();
    let rgb_pixels = demosaic_xtrans_markesteijn(&xtrans);
    let demosaic_elapsed = demosaic_start.elapsed();

    tracing::info!(
        "X-Trans Markesteijn demosaicing {}x{} took {:.2}ms",
        width,
        height,
        demosaic_elapsed.as_secs_f64() * 1000.0
    );

    rgb_pixels
}

/// Process pre-normalized f32 X-Trans data and demosaic to RGB.
///
/// Takes f32 data already in [0,1] range (e.g., from CfaImage after calibration).
/// Avoids the lossy f32->u16->f32 roundtrip of converting to u16 for `process_xtrans`.
#[allow(clippy::too_many_arguments)]
pub fn process_xtrans_f32(
    data: &[f32],
    raw_width: usize,
    raw_height: usize,
    width: usize,
    height: usize,
    top_margin: usize,
    left_margin: usize,
    xtrans_pattern: [[u8; 6]; 6],
) -> Vec<f32> {
    let pattern = XTransPattern::new(xtrans_pattern);

    let xtrans = XTransImage::with_margins_f32(
        data,
        raw_width,
        raw_height,
        width,
        height,
        top_margin,
        left_margin,
        pattern,
    );

    let demosaic_start = Instant::now();
    let rgb_pixels = demosaic_xtrans_markesteijn(&xtrans);
    let demosaic_elapsed = demosaic_start.elapsed();

    tracing::info!(
        "X-Trans Markesteijn demosaicing (f32) {}x{} took {:.2}ms",
        width,
        height,
        demosaic_elapsed.as_secs_f64() * 1000.0
    );

    rgb_pixels
}

/// X-Trans 6x6 color filter array pattern.
/// Values: 0=Red, 1=Green, 2=Blue
#[derive(Debug, Clone)]
pub(crate) struct XTransPattern {
    /// 6x6 pattern array indexed by [row % 6][col % 6]
    pub(crate) pattern: [[u8; 6]; 6],
}

impl XTransPattern {
    /// Create a new X-Trans pattern from a 6x6 array.
    ///
    /// # Panics
    /// Panics if any value in the pattern is not 0, 1, or 2.
    pub(crate) fn new(pattern: [[u8; 6]; 6]) -> Self {
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
    pub(crate) fn color_at(&self, row: usize, col: usize) -> u8 {
        self.pattern[row % 6][col % 6]
    }
}

/// Pixel data source: either raw u16 sensor values or pre-normalized f32.
///
/// The u16 path is used by the raw loader (saves ~47 MB by deferring normalization).
/// The f32 path is used by CfaImage after calibration (avoids lossy f32->u16 roundtrip).
#[derive(Debug)]
pub(crate) enum PixelSource<'a> {
    U16(&'a [u16]),
    F32(&'a [f32]),
}

/// Raw X-Trans image data with metadata needed for demosaicing.
///
/// Supports both raw u16 sensor data (with on-the-fly normalization) and
/// pre-normalized f32 data (identity passthrough).
#[derive(Debug)]
pub(crate) struct XTransImage<'a> {
    /// Pixel data (u16 raw sensor values or pre-normalized f32)
    pub(crate) data: PixelSource<'a>,
    /// Width of the raw data buffer
    pub(crate) raw_width: usize,
    /// Height of the raw data buffer
    pub(crate) raw_height: usize,
    /// Width of the active/output image area
    pub(crate) width: usize,
    /// Height of the active/output image area
    pub(crate) height: usize,
    /// Top margin (offset from raw to active area)
    pub(crate) top_margin: usize,
    /// Left margin (offset from raw to active area)
    pub(crate) left_margin: usize,
    /// X-Trans CFA pattern
    pub(crate) pattern: XTransPattern,
    /// Black level for normalization (u16 path only)
    black: f32,
    /// 1.0 / (maximum - black) for normalization (u16 path only)
    inv_range: f32,
}

impl<'a> XTransImage<'a> {
    /// Validate dimensions and margins (shared by both constructors).
    fn validate_dimensions(
        data_len: usize,
        raw_width: usize,
        raw_height: usize,
        width: usize,
        height: usize,
        top_margin: usize,
        left_margin: usize,
    ) {
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
            data_len == raw_width * raw_height,
            "Data length {} doesn't match raw dimensions {}x{}={}",
            data_len,
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
    }

    /// Create from raw u16 sensor data with on-the-fly normalization.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn with_margins(
        data: &'a [u16],
        raw_width: usize,
        raw_height: usize,
        width: usize,
        height: usize,
        top_margin: usize,
        left_margin: usize,
        pattern: XTransPattern,
        black: f32,
        inv_range: f32,
    ) -> Self {
        Self::validate_dimensions(
            data.len(),
            raw_width,
            raw_height,
            width,
            height,
            top_margin,
            left_margin,
        );
        Self {
            data: PixelSource::U16(data),
            raw_width,
            raw_height,
            width,
            height,
            top_margin,
            left_margin,
            pattern,
            black,
            inv_range,
        }
    }

    /// Create from pre-normalized f32 data (already in [0,1] range).
    ///
    /// Used by CfaImage after calibration to avoid lossy f32->u16->f32 roundtrip.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn with_margins_f32(
        data: &'a [f32],
        raw_width: usize,
        raw_height: usize,
        width: usize,
        height: usize,
        top_margin: usize,
        left_margin: usize,
        pattern: XTransPattern,
    ) -> Self {
        Self::validate_dimensions(
            data.len(),
            raw_width,
            raw_height,
            width,
            height,
            top_margin,
            left_margin,
        );
        Self {
            data: PixelSource::F32(data),
            raw_width,
            raw_height,
            width,
            height,
            top_margin,
            left_margin,
            pattern,
            black: 0.0,
            inv_range: 1.0,
        }
    }

    /// Read a pixel and return its normalized value in 0.0-1.0 range.
    ///
    /// For u16 data: `((value - black).max(0) * inv_range)`
    /// For f32 data: returns value directly (already normalized).
    #[inline(always)]
    pub(crate) fn read_normalized(&self, raw_y: usize, raw_x: usize) -> f32 {
        let idx = raw_y * self.raw_width + raw_x;
        match &self.data {
            PixelSource::U16(data) => {
                let val = data[idx] as f32;
                (val - self.black).max(0.0) * self.inv_range
            }
            PixelSource::F32(data) => data[idx],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Standard X-Trans pattern (one common variant)
    fn test_pattern_array() -> [[u8; 6]; 6] {
        [
            [1, 0, 1, 1, 2, 1], // G R G G B G
            [2, 1, 2, 0, 1, 0], // B G B R G R
            [1, 2, 1, 1, 0, 1], // G B G G R G
            [1, 2, 1, 1, 0, 1], // G B G G R G
            [0, 1, 0, 2, 1, 2], // R G R B G B
            [1, 0, 1, 1, 2, 1], // G R G G B G
        ]
    }

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
        let data = vec![32768u16; 36];
        let pattern = test_pattern();
        let img = XTransImage::with_margins(&data, 6, 6, 4, 4, 1, 1, pattern, 0.0, 1.0 / 65535.0);
        assert_eq!(img.raw_width, 6);
        assert_eq!(img.raw_height, 6);
        assert_eq!(img.width, 4);
        assert_eq!(img.height, 4);
    }

    #[test]
    #[should_panic(expected = "Output dimensions must be non-zero")]
    fn test_xtrans_image_zero_width() {
        let data = vec![32768u16; 36];
        let pattern = test_pattern();
        XTransImage::with_margins(&data, 6, 6, 0, 4, 0, 0, pattern, 0.0, 1.0 / 65535.0);
    }

    #[test]
    #[should_panic(expected = "Data length")]
    fn test_xtrans_image_wrong_data_length() {
        let data = vec![32768u16; 30]; // Should be 36
        let pattern = test_pattern();
        XTransImage::with_margins(&data, 6, 6, 6, 6, 0, 0, pattern, 0.0, 1.0 / 65535.0);
    }

    #[test]
    fn test_process_xtrans_output_size() {
        let raw_data: Vec<u16> = vec![1000; 12 * 12];
        let rgb = process_xtrans(
            &raw_data,
            12,
            12,
            6,
            6,
            3,
            3,
            test_pattern_array(),
            0.0,
            1.0 / 4096.0,
        );

        assert_eq!(rgb.len(), 6 * 6 * 3);
    }

    #[test]
    fn test_process_xtrans_normalization() {
        let black = 256.0;
        let maximum = 4096.0;
        let range = maximum - black;
        let inv_range = 1.0 / range;

        // All values equal black + range/2 = 2176 → normalizes to 0.5
        let mid_value = (black + range / 2.0) as u16;
        let raw_data: Vec<u16> = vec![mid_value; 12 * 12];

        let rgb = process_xtrans(
            &raw_data,
            12,
            12,
            6,
            6,
            3,
            3,
            test_pattern_array(),
            black,
            inv_range,
        );

        for &val in &rgb {
            assert!((val - 0.5).abs() < 0.01, "Expected ~0.5, got {}", val);
        }
    }

    #[test]
    fn test_process_xtrans_clamps_below_black() {
        let black = 256.0;
        let range = 4096.0 - black;
        let inv_range = 1.0 / range;

        // All values below black level
        let raw_data: Vec<u16> = vec![100; 12 * 12];

        let rgb = process_xtrans(
            &raw_data,
            12,
            12,
            6,
            6,
            3,
            3,
            test_pattern_array(),
            black,
            inv_range,
        );

        for &val in &rgb {
            assert_eq!(val, 0.0, "Expected 0.0 for values below black level");
        }
    }

    #[test]
    fn test_process_xtrans_full_range() {
        let black = 0.0;
        let inv_range = 1.0 / 65535.0;

        let raw_data: Vec<u16> = vec![65535; 12 * 12];

        let rgb = process_xtrans(
            &raw_data,
            12,
            12,
            6,
            6,
            3,
            3,
            test_pattern_array(),
            black,
            inv_range,
        );

        for &val in &rgb {
            assert!((val - 1.0).abs() < 0.001, "Expected 1.0, got {}", val);
        }
    }

    // ---------------------------------------------------------------
    // process_xtrans_f32 tests
    // ---------------------------------------------------------------

    #[test]
    fn test_process_xtrans_f32_output_size() {
        let data: Vec<f32> = vec![0.5; 12 * 12];
        let rgb = process_xtrans_f32(&data, 12, 12, 6, 6, 3, 3, test_pattern_array());
        assert_eq!(rgb.len(), 6 * 6 * 3);
    }

    #[test]
    fn test_process_xtrans_f32_uniform() {
        let data: Vec<f32> = vec![0.5; 12 * 12];
        let rgb = process_xtrans_f32(&data, 12, 12, 6, 6, 3, 3, test_pattern_array());

        for &val in &rgb {
            assert!((val - 0.5).abs() < 0.01, "Expected ~0.5, got {}", val);
        }
    }

    #[test]
    fn test_process_xtrans_f32_matches_u16_path() {
        // Verify f32 path produces equivalent output to u16 path for the same input.
        let black = 0.0_f32;
        let inv_range = 1.0 / 65535.0_f32;

        // Create u16 data and equivalent normalized f32 data
        let raw_u16: Vec<u16> = (0..12 * 12).map(|i| (i * 400 + 1000) as u16).collect();
        let raw_f32: Vec<f32> = raw_u16
            .iter()
            .map(|&v| (v as f32 - black).max(0.0) * inv_range)
            .collect();

        let rgb_u16 = process_xtrans(
            &raw_u16,
            12,
            12,
            6,
            6,
            3,
            3,
            test_pattern_array(),
            black,
            inv_range,
        );
        let rgb_f32 = process_xtrans_f32(&raw_f32, 12, 12, 6, 6, 3, 3, test_pattern_array());

        assert_eq!(rgb_u16.len(), rgb_f32.len());
        for (i, (&a, &b)) in rgb_u16.iter().zip(rgb_f32.iter()).enumerate() {
            assert!(
                (a - b).abs() < 1e-5,
                "Pixel {i}: u16 path={a}, f32 path={b}, diff={}",
                (a - b).abs()
            );
        }
    }
}
