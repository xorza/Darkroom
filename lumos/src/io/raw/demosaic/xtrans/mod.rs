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

use common::CancelToken;
use markesteijn::demosaic_xtrans_markesteijn;

use crate::io::raw::demosaic::Cancelled;

/// Process X-Trans sensor data and demosaic to RGB.
///
/// Takes raw u16 sensor data and normalization parameters. Normalization happens
/// on-the-fly during demosaicing, avoiding a separate P×4 byte f32 buffer.
///
/// Returns planar `[R, G, B]` channels, each `width * height`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn process_xtrans(
    raw_data: &[u16],
    raw_width: usize,
    raw_height: usize,
    width: usize,
    height: usize,
    top_margin: usize,
    left_margin: usize,
    raw_pattern: [[u8; 6]; 6],
    channel_black: [f32; 3],
    inv_range: f32,
) -> [Vec<f32>; 3] {
    let raw_pattern = XTransPattern::new(raw_pattern);

    let xtrans = XTransImage::with_margins(
        raw_data,
        raw_width,
        raw_height,
        width,
        height,
        top_margin,
        left_margin,
        raw_pattern,
        channel_black,
        inv_range,
    );

    let demosaic_start = Instant::now();
    // The u16 decode path isn't cancellable — a never-token can't yield `Cancelled`.
    let rgb_pixels = demosaic_xtrans_markesteijn(&xtrans, &CancelToken::never())
        .expect("never-token demosaic cannot be cancelled");
    let demosaic_elapsed = demosaic_start.elapsed();

    tracing::info!(
        "X-Trans Markesteijn demosaicing {}x{} took {:.2}ms",
        width,
        height,
        demosaic_elapsed.as_secs_f64() * 1000.0
    );

    rgb_pixels
}

/// Process calibrated f32 X-Trans data and demosaic to RGB.
///
/// Avoids the lossy f32->u16->f32 roundtrip of converting to u16 for `process_xtrans`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn process_xtrans_f32(
    data: &[f32],
    raw_width: usize,
    raw_height: usize,
    width: usize,
    height: usize,
    top_margin: usize,
    left_margin: usize,
    raw_pattern: [[u8; 6]; 6],
    cancel: &CancelToken,
) -> Result<[Vec<f32>; 3], Cancelled> {
    let raw_pattern = XTransPattern::new(raw_pattern);

    let xtrans = XTransImage::with_margins_f32(
        data,
        raw_width,
        raw_height,
        width,
        height,
        top_margin,
        left_margin,
        raw_pattern,
    );

    let demosaic_start = Instant::now();
    let rgb_pixels = demosaic_xtrans_markesteijn(&xtrans, cancel)?;
    let demosaic_elapsed = demosaic_start.elapsed();

    tracing::info!(
        "X-Trans Markesteijn demosaicing (f32) {}x{} took {:.2}ms",
        width,
        height,
        demosaic_elapsed.as_secs_f64() * 1000.0
    );

    Ok(rgb_pixels)
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
    /// Panics unless the values, color counts, and green neighborhoods form an X-Trans CFA.
    pub(crate) fn new(pattern: [[u8; 6]; 6]) -> Self {
        let mut counts = [0usize; 3];
        for row in &pattern {
            for &val in row {
                assert!(
                    val <= 2,
                    "Invalid X-Trans pattern value: {} (must be 0, 1, or 2)",
                    val
                );
                counts[val as usize] += 1;
            }
        }
        assert_eq!(
            counts,
            [8, 20, 8],
            "Invalid X-Trans color counts: expected [8 red, 20 green, 8 blue], got {counts:?}"
        );
        for row in 0..6 {
            for col in 0..6 {
                if pattern[row][col] != 1 {
                    continue;
                }
                let mut neighbors = [0usize; 3];
                for (dy, dx) in [(0, 1), (1, 0), (0, -1), (-1, 0)] {
                    let y = (row as i32 + dy).rem_euclid(6) as usize;
                    let x = (col as i32 + dx).rem_euclid(6) as usize;
                    neighbors[pattern[y][x] as usize] += 1;
                }
                assert_eq!(
                    neighbors[0], neighbors[2],
                    "Invalid X-Trans green neighborhood at ({row},{col}): {neighbors:?}"
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

/// Pixel data source: either raw u16 sensor values or calibrated f32.
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
/// calibrated f32 data (identity passthrough).
#[derive(Debug)]
pub(crate) struct XTransImage<'a> {
    /// Pixel data (u16 raw sensor values or calibrated f32)
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
    /// CFA pattern anchored at the full raw buffer origin.
    pub(crate) raw_pattern: XTransPattern,
    /// Per-channel black levels [R=0, G=1, B=2] for u16 path normalization.
    channel_black: [f32; 3],
    /// 1.0 / (maximum - common_black) for normalization (u16 path only).
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

    /// Create from raw u16 sensor data with on-the-fly per-channel normalization.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn with_margins(
        data: &'a [u16],
        raw_width: usize,
        raw_height: usize,
        width: usize,
        height: usize,
        top_margin: usize,
        left_margin: usize,
        raw_pattern: XTransPattern,
        channel_black: [f32; 3],
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
            raw_pattern,
            channel_black,
            inv_range,
        }
    }

    /// Create from calibrated f32 data, including negative and above-unity samples.
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
        raw_pattern: XTransPattern,
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
            raw_pattern,
            channel_black: [0.0; 3],
            inv_range: 1.0,
        }
    }

    /// Read a pixel and return its normalized raw-linear value.
    ///
    /// For u16 data: per-channel black subtraction and normalization.
    /// For f32 data: returns the calibrated value directly.
    #[inline(always)]
    pub(crate) fn read_normalized(&self, raw_y: usize, raw_x: usize) -> f32 {
        let idx = raw_y * self.raw_width + raw_x;
        match &self.data {
            PixelSource::U16(data) => {
                let val = data[idx] as f32;
                let ch = self.raw_pattern.color_at(raw_y, raw_x) as usize;
                ((val - self.channel_black[ch]).max(0.0) * self.inv_range).min(1.0)
            }
            PixelSource::F32(data) => data[idx],
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::io::raw::demosaic::xtrans::test_support::{test_pattern, test_pattern_array};
    use crate::io::raw::demosaic::xtrans::*;

    #[test]
    fn test_xtrans_pattern_color_at() {
        let pattern = test_pattern();
        // Check corners
        assert_eq!(pattern.color_at(0, 0), 1); // G
        assert_eq!(pattern.color_at(0, 2), 0); // R
        assert_eq!(pattern.color_at(0, 5), 2); // B
        assert_eq!(pattern.color_at(2, 0), 2); // B
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
    #[should_panic(expected = "Invalid X-Trans color counts")]
    fn test_xtrans_pattern_invalid_color_counts() {
        let mut pattern = test_pattern_array();
        pattern[0][2] = 1;
        XTransPattern::new(pattern);
    }

    #[test]
    #[should_panic(expected = "Invalid X-Trans green neighborhood")]
    fn test_xtrans_pattern_invalid_geometry() {
        XTransPattern::new([
            [1, 0, 1, 1, 2, 1],
            [2, 1, 2, 0, 1, 0],
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
        let img =
            XTransImage::with_margins(&data, 6, 6, 4, 4, 1, 1, pattern, [0.0; 3], 1.0 / 65535.0);
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
        XTransImage::with_margins(&data, 6, 6, 0, 4, 0, 0, pattern, [0.0; 3], 1.0 / 65535.0);
    }

    #[test]
    #[should_panic(expected = "Data length")]
    fn test_xtrans_image_wrong_data_length() {
        let data = vec![32768u16; 30]; // Should be 36
        let pattern = test_pattern();
        XTransImage::with_margins(&data, 6, 6, 6, 6, 0, 0, pattern, [0.0; 3], 1.0 / 65535.0);
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
            [0.0; 3],
            1.0 / 4096.0,
        );

        assert_eq!(rgb.iter().map(|c| c.len()).sum::<usize>(), 6 * 6 * 3);
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
            [black; 3],
            inv_range,
        );

        for &val in rgb.iter().flatten() {
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
            [black; 3],
            inv_range,
        );

        for &val in rgb.iter().flatten() {
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
            [black; 3],
            inv_range,
        );

        for &val in rgb.iter().flatten() {
            assert!((val - 1.0).abs() < 0.001, "Expected 1.0, got {}", val);
        }
    }

    #[test]
    fn test_xtrans_normalization_is_per_channel_and_raw_linear() {
        let common_black = 200.0;
        let maximum = 4096.0;
        let inv_range = 1.0 / (maximum - common_black);
        let raw_val = 2000u16;
        let raw_data = vec![raw_val; 6 * 6];
        let image = XTransImage::with_margins(
            &raw_data,
            6,
            6,
            6,
            6,
            0,
            0,
            test_pattern(),
            [250.0, common_black, 220.0],
            inv_range,
        );

        let expected_red = (2000.0 - 250.0) / 3896.0;
        let expected_green = (2000.0 - 200.0) / 3896.0;
        let expected_blue = (2000.0 - 220.0) / 3896.0;
        assert!((image.read_normalized(0, 2) - expected_red).abs() < 1e-7);
        assert!((image.read_normalized(0, 0) - expected_green).abs() < 1e-7);
        assert!((image.read_normalized(0, 5) - expected_blue).abs() < 1e-7);
    }

    #[test]
    fn test_process_xtrans_f32_output_size() {
        let data: Vec<f32> = vec![0.5; 12 * 12];
        let rgb = process_xtrans_f32(
            &data,
            12,
            12,
            6,
            6,
            3,
            3,
            test_pattern_array(),
            &CancelToken::never(),
        )
        .unwrap();
        assert_eq!(rgb.iter().map(|c| c.len()).sum::<usize>(), 6 * 6 * 3);
    }

    #[test]
    fn test_process_xtrans_f32_uniform() {
        let data: Vec<f32> = vec![0.5; 12 * 12];
        let rgb = process_xtrans_f32(
            &data,
            12,
            12,
            6,
            6,
            3,
            3,
            test_pattern_array(),
            &CancelToken::never(),
        )
        .unwrap();

        for &val in rgb.iter().flatten() {
            assert!((val - 0.5).abs() < 0.01, "Expected ~0.5, got {}", val);
        }
    }

    #[test]
    fn f32_demosaic_preserves_signed_native_samples() {
        let raw_width = 30;
        let raw_height = 30;
        let width = 18;
        let height = 18;
        let top = 6;
        let left = 6;
        let pattern = test_pattern_array();
        let data: Vec<f32> = (0..raw_width * raw_height)
            .map(|index| (index % 17) as f32 * 0.25 - 2.0)
            .collect();

        let rgb = process_xtrans_f32(
            &data,
            raw_width,
            raw_height,
            width,
            height,
            top,
            left,
            pattern,
            &CancelToken::never(),
        )
        .unwrap();

        for y in 0..height {
            for x in 0..width {
                let raw_y = y + top;
                let raw_x = x + left;
                let channel = pattern[raw_y % 6][raw_x % 6] as usize;
                let expected = data[raw_y * raw_width + raw_x];
                let actual = rgb[channel][y * width + x];
                assert!(
                    (actual - expected).abs() < 1e-6,
                    "native channel {channel} at ({x}, {y}) changed from {expected} to {actual}"
                );
            }
        }
    }

    #[test]
    fn f32_demosaic_is_equivariant_to_a_uniform_pedestal() {
        let raw_width = 30;
        let raw_height = 30;
        let width = 18;
        let height = 18;
        let top = 6;
        let left = 6;
        let pedestal = 0.375;
        let base: Vec<f32> = (0..raw_width * raw_height)
            .map(|index| 0.2 + (index * 37 % 101) as f32 / 200.0)
            .collect();
        let shifted: Vec<f32> = base.iter().map(|value| value + pedestal).collect();
        let demosaic = |data: &[f32]| {
            process_xtrans_f32(
                data,
                raw_width,
                raw_height,
                width,
                height,
                top,
                left,
                test_pattern_array(),
                &CancelToken::never(),
            )
            .unwrap()
        };

        let base_rgb = demosaic(&base);
        let shifted_rgb = demosaic(&shifted);
        for (channel, (base_channel, shifted_channel)) in
            base_rgb.iter().zip(&shifted_rgb).enumerate()
        {
            for (pixel, (&base_value, &shifted_value)) in
                base_channel.iter().zip(shifted_channel).enumerate()
            {
                assert!(
                    (shifted_value - base_value - pedestal).abs() < 2e-6,
                    "channel {channel} pixel {pixel}: expected pedestal {pedestal}, got {}",
                    shifted_value - base_value
                );
            }
        }
    }

    #[test]
    fn test_process_xtrans_f32_matches_u16_path() {
        let black = 0.0_f32;
        let inv_range = 1.0 / 65535.0_f32;
        let raw_width = 30;
        let raw_height = 30;
        let width = 18;
        let height = 18;
        let margin = 6;
        let raw_u16: Vec<u16> = (0..raw_width * raw_height)
            .map(|index| {
                let y = index / raw_width;
                let x = index % raw_width;
                if (x / 2 + y / 3) % 2 == 0 {
                    512
                } else {
                    64_000
                }
            })
            .collect();
        let raw_f32: Vec<f32> = raw_u16
            .iter()
            .map(|&v| (v as f32 - black).max(0.0) * inv_range)
            .collect();

        let rgb_u16 = process_xtrans(
            &raw_u16,
            raw_width,
            raw_height,
            width,
            height,
            margin,
            margin,
            test_pattern_array(),
            [black; 3],
            inv_range,
        );
        let rgb_f32 = process_xtrans_f32(
            &raw_f32,
            raw_width,
            raw_height,
            width,
            height,
            margin,
            margin,
            test_pattern_array(),
            &CancelToken::never(),
        )
        .unwrap();

        assert_eq!(
            rgb_u16.iter().flatten().count(),
            rgb_f32.iter().flatten().count()
        );
        for (i, (&a, &b)) in rgb_u16
            .iter()
            .flatten()
            .zip(rgb_f32.iter().flatten())
            .enumerate()
        {
            assert!(
                (a - b).abs() < 1e-5,
                "Pixel {i}: u16 path={a}, f32 path={b}, diff={}",
                (a - b).abs()
            );
        }
        assert!(
            rgb_u16
                .iter()
                .flatten()
                .any(|value| !(0.0..=1.0).contains(value)),
            "high-contrast interpolation should retain a legitimate overshoot"
        );
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use crate::io::raw::demosaic::xtrans::{XTransImage, XTransPattern};

    const TEST_PATTERN: [[u8; 6]; 6] = [
        [1, 1, 0, 1, 1, 2],
        [1, 1, 2, 1, 1, 0],
        [2, 0, 1, 0, 2, 1],
        [1, 1, 2, 1, 1, 0],
        [1, 1, 0, 1, 1, 2],
        [0, 2, 1, 2, 0, 1],
    ];

    pub(crate) const TEST_INV_RANGE: f32 = 1.0 / 65535.0;

    pub(crate) fn test_pattern_array() -> [[u8; 6]; 6] {
        TEST_PATTERN
    }

    pub(crate) fn test_pattern() -> XTransPattern {
        XTransPattern::new(TEST_PATTERN)
    }

    pub(crate) fn to_u16(value: f32) -> u16 {
        (value * 65535.0).round() as u16
    }

    pub(crate) fn make_xtrans(
        data: &[u16],
        raw_width: usize,
        raw_height: usize,
        width: usize,
        height: usize,
        top_margin: usize,
        left_margin: usize,
    ) -> XTransImage<'_> {
        XTransImage::with_margins(
            data,
            raw_width,
            raw_height,
            width,
            height,
            top_margin,
            left_margin,
            test_pattern(),
            [0.0; 3],
            TEST_INV_RANGE,
        )
    }
}
