/// Bayer CFA (Color Filter Array) pattern.
/// Represents the 2x2 pattern of color filters on the sensor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum CfaPattern {
    /// RGGB: Red at (0,0), Green at (0,1) and (1,0), Blue at (1,1)
    Rggb,
    /// BGGR: Blue at (0,0), Green at (0,1) and (1,0), Red at (1,1)
    Bggr,
    /// GRBG: Green at (0,0), Red at (0,1), Blue at (1,0), Green at (1,1)
    Grbg,
    /// GBRG: Green at (0,0), Blue at (0,1), Red at (1,0), Green at (1,1)
    Gbrg,
}

impl CfaPattern {
    /// Get color index at position (y, x) in the Bayer pattern.
    /// Returns: 0=Red, 1=Green, 2=Blue
    #[inline]
    pub fn color_at(&self, y: usize, x: usize) -> usize {
        let row = y % 2;
        let col = x % 2;
        match self {
            CfaPattern::Rggb => match (row, col) {
                (0, 0) => 0, // R
                (0, 1) => 1, // G
                (1, 0) => 1, // G
                (1, 1) => 2, // B
                _ => unreachable!(),
            },
            CfaPattern::Bggr => match (row, col) {
                (0, 0) => 2, // B
                (0, 1) => 1, // G
                (1, 0) => 1, // G
                (1, 1) => 0, // R
                _ => unreachable!(),
            },
            CfaPattern::Grbg => match (row, col) {
                (0, 0) => 1, // G
                (0, 1) => 0, // R
                (1, 0) => 2, // B
                (1, 1) => 1, // G
                _ => unreachable!(),
            },
            CfaPattern::Gbrg => match (row, col) {
                (0, 0) => 1, // G
                (0, 1) => 2, // B
                (1, 0) => 0, // R
                (1, 1) => 1, // G
                _ => unreachable!(),
            },
        }
    }

    /// Check if red is on the same row as a green pixel at position (y, x).
    /// Used to determine interpolation direction for green pixels.
    #[inline]
    pub fn red_in_row(&self, y: usize) -> bool {
        match self {
            CfaPattern::Rggb | CfaPattern::Grbg => y.is_multiple_of(2),
            CfaPattern::Bggr | CfaPattern::Gbrg => !y.is_multiple_of(2),
        }
    }
}

/// Raw Bayer image data with metadata needed for demosaicing.
#[derive(Debug)]
pub struct BayerImage<'a> {
    /// Raw Bayer pixel data (normalized to 0.0-1.0)
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
    /// CFA pattern
    pub cfa: CfaPattern,
}

impl<'a> BayerImage<'a> {
    /// Create a BayerImage with margins (libraw style).
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
        cfa: CfaPattern,
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
            cfa,
        }
    }
}

/// Simple bilinear demosaicing of Bayer CFA data to RGB.
///
/// Takes a Bayer pattern image and produces an RGB image using bilinear interpolation.
/// The output has 3 channels (RGB) interleaved: [R0, G0, B0, R1, G1, B1, ...].
pub fn demosaic_bilinear(bayer: &BayerImage) -> Vec<f32> {
    let mut rgb = vec![0.0f32; bayer.width * bayer.height * 3];

    for y in 0..bayer.height {
        for x in 0..bayer.width {
            // Map to raw coordinates
            let raw_y = y + bayer.top_margin;
            let raw_x = x + bayer.left_margin;

            let color = bayer.cfa.color_at(raw_y, raw_x);
            let rgb_idx = (y * bayer.width + x) * 3;

            // Get the value at this pixel in raw coordinates
            let val = bayer.data[raw_y * bayer.raw_width + raw_x];

            match color {
                0 => {
                    // Red pixel - interpolate G and B
                    rgb[rgb_idx] = val;
                    rgb[rgb_idx + 1] = interpolate_cross(bayer, raw_x, raw_y);
                    rgb[rgb_idx + 2] = interpolate_diagonal(bayer, raw_x, raw_y);
                }
                1 => {
                    // Green pixel - interpolate R and B
                    if bayer.cfa.red_in_row(raw_y) {
                        rgb[rgb_idx] = interpolate_horizontal(bayer, raw_x, raw_y);
                        rgb[rgb_idx + 2] = interpolate_vertical(bayer, raw_x, raw_y);
                    } else {
                        rgb[rgb_idx] = interpolate_vertical(bayer, raw_x, raw_y);
                        rgb[rgb_idx + 2] = interpolate_horizontal(bayer, raw_x, raw_y);
                    }
                    rgb[rgb_idx + 1] = val;
                }
                2 => {
                    // Blue pixel - interpolate R and G
                    rgb[rgb_idx] = interpolate_diagonal(bayer, raw_x, raw_y);
                    rgb[rgb_idx + 1] = interpolate_cross(bayer, raw_x, raw_y);
                    rgb[rgb_idx + 2] = val;
                }
                _ => unreachable!(),
            }
        }
    }

    rgb
}

/// Interpolate from horizontal neighbors.
#[inline]
fn interpolate_horizontal(bayer: &BayerImage, x: usize, y: usize) -> f32 {
    let idx = y * bayer.raw_width + x;
    let left = if x > 0 {
        bayer.data[idx - 1]
    } else {
        bayer.data[idx + 1]
    };
    let right = if x + 1 < bayer.raw_width {
        bayer.data[idx + 1]
    } else {
        bayer.data[idx - 1]
    };
    (left + right) * 0.5
}

/// Interpolate from vertical neighbors.
#[inline]
fn interpolate_vertical(bayer: &BayerImage, x: usize, y: usize) -> f32 {
    let idx = y * bayer.raw_width + x;
    let top = if y > 0 {
        bayer.data[idx - bayer.raw_width]
    } else {
        bayer.data[idx + bayer.raw_width]
    };
    let bottom = if y + 1 < bayer.raw_height {
        bayer.data[idx + bayer.raw_width]
    } else {
        bayer.data[idx - bayer.raw_width]
    };
    (top + bottom) * 0.5
}

/// Interpolate from cross (4 neighbors).
#[inline]
fn interpolate_cross(bayer: &BayerImage, x: usize, y: usize) -> f32 {
    let idx = y * bayer.raw_width + x;
    let mut sum = 0.0f32;
    let mut count = 0;

    if x > 0 {
        sum += bayer.data[idx - 1];
        count += 1;
    }
    if x + 1 < bayer.raw_width {
        sum += bayer.data[idx + 1];
        count += 1;
    }
    if y > 0 {
        sum += bayer.data[idx - bayer.raw_width];
        count += 1;
    }
    if y + 1 < bayer.raw_height {
        sum += bayer.data[idx + bayer.raw_width];
        count += 1;
    }

    if count > 0 {
        sum / count as f32
    } else {
        bayer.data[idx]
    }
}

/// Interpolate from diagonal neighbors.
#[inline]
fn interpolate_diagonal(bayer: &BayerImage, x: usize, y: usize) -> f32 {
    let idx = y * bayer.raw_width + x;
    let mut sum = 0.0f32;
    let mut count = 0;

    if x > 0 && y > 0 {
        sum += bayer.data[idx - bayer.raw_width - 1];
        count += 1;
    }
    if x + 1 < bayer.raw_width && y > 0 {
        sum += bayer.data[idx - bayer.raw_width + 1];
        count += 1;
    }
    if x > 0 && y + 1 < bayer.raw_height {
        sum += bayer.data[idx + bayer.raw_width - 1];
        count += 1;
    }
    if x + 1 < bayer.raw_width && y + 1 < bayer.raw_height {
        sum += bayer.data[idx + bayer.raw_width + 1];
        count += 1;
    }

    if count > 0 {
        sum / count as f32
    } else {
        bayer.data[idx]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Test CFA patterns
    #[test]
    fn test_cfa_rggb_pattern() {
        let cfa = CfaPattern::Rggb;
        // Row 0: R G R G ...
        assert_eq!(cfa.color_at(0, 0), 0); // R
        assert_eq!(cfa.color_at(0, 1), 1); // G
        assert_eq!(cfa.color_at(0, 2), 0); // R
        assert_eq!(cfa.color_at(0, 3), 1); // G
        // Row 1: G B G B ...
        assert_eq!(cfa.color_at(1, 0), 1); // G
        assert_eq!(cfa.color_at(1, 1), 2); // B
        assert_eq!(cfa.color_at(1, 2), 1); // G
        assert_eq!(cfa.color_at(1, 3), 2); // B
    }

    #[test]
    fn test_cfa_bggr_pattern() {
        let cfa = CfaPattern::Bggr;
        // Row 0: B G B G ...
        assert_eq!(cfa.color_at(0, 0), 2); // B
        assert_eq!(cfa.color_at(0, 1), 1); // G
        // Row 1: G R G R ...
        assert_eq!(cfa.color_at(1, 0), 1); // G
        assert_eq!(cfa.color_at(1, 1), 0); // R
    }

    #[test]
    fn test_cfa_grbg_pattern() {
        let cfa = CfaPattern::Grbg;
        // Row 0: G R G R ...
        assert_eq!(cfa.color_at(0, 0), 1); // G
        assert_eq!(cfa.color_at(0, 1), 0); // R
        // Row 1: B G B G ...
        assert_eq!(cfa.color_at(1, 0), 2); // B
        assert_eq!(cfa.color_at(1, 1), 1); // G
    }

    #[test]
    fn test_cfa_gbrg_pattern() {
        let cfa = CfaPattern::Gbrg;
        // Row 0: G B G B ...
        assert_eq!(cfa.color_at(0, 0), 1); // G
        assert_eq!(cfa.color_at(0, 1), 2); // B
        // Row 1: R G R G ...
        assert_eq!(cfa.color_at(1, 0), 0); // R
        assert_eq!(cfa.color_at(1, 1), 1); // G
    }

    #[test]
    fn test_red_in_row() {
        // RGGB: Red in row 0, 2, 4, ...
        assert!(CfaPattern::Rggb.red_in_row(0));
        assert!(!CfaPattern::Rggb.red_in_row(1));
        assert!(CfaPattern::Rggb.red_in_row(2));

        // BGGR: Red in row 1, 3, 5, ...
        assert!(!CfaPattern::Bggr.red_in_row(0));
        assert!(CfaPattern::Bggr.red_in_row(1));
        assert!(!CfaPattern::Bggr.red_in_row(2));

        // GRBG: Red in row 0, 2, 4, ...
        assert!(CfaPattern::Grbg.red_in_row(0));
        assert!(!CfaPattern::Grbg.red_in_row(1));

        // GBRG: Red in row 1, 3, 5, ...
        assert!(!CfaPattern::Gbrg.red_in_row(0));
        assert!(CfaPattern::Gbrg.red_in_row(1));
    }

    // Test BayerImage validation
    #[test]
    #[should_panic(expected = "Output dimensions must be non-zero")]
    fn test_bayer_image_zero_width() {
        let data = vec![0.0f32; 4];
        BayerImage::with_margins(&data, 2, 2, 0, 2, 0, 0, CfaPattern::Rggb);
    }

    #[test]
    #[should_panic(expected = "Output dimensions must be non-zero")]
    fn test_bayer_image_zero_height() {
        let data = vec![0.0f32; 4];
        BayerImage::with_margins(&data, 2, 2, 2, 0, 0, 0, CfaPattern::Rggb);
    }

    #[test]
    #[should_panic(expected = "Data length")]
    fn test_bayer_image_wrong_data_length() {
        let data = vec![0.0f32; 3]; // Should be 4
        BayerImage::with_margins(&data, 2, 2, 2, 2, 0, 0, CfaPattern::Rggb);
    }

    #[test]
    #[should_panic(expected = "Top margin")]
    fn test_bayer_image_margin_exceeds_height() {
        let data = vec![0.0f32; 4];
        BayerImage::with_margins(&data, 2, 2, 2, 2, 1, 0, CfaPattern::Rggb);
    }

    #[test]
    #[should_panic(expected = "Left margin")]
    fn test_bayer_image_margin_exceeds_width() {
        let data = vec![0.0f32; 4];
        BayerImage::with_margins(&data, 2, 2, 2, 2, 0, 1, CfaPattern::Rggb);
    }

    #[test]
    fn test_bayer_image_valid() {
        let data = vec![0.0f32; 16];
        let bayer = BayerImage::with_margins(&data, 4, 4, 2, 2, 1, 1, CfaPattern::Rggb);
        assert_eq!(bayer.raw_width, 4);
        assert_eq!(bayer.raw_height, 4);
        assert_eq!(bayer.width, 2);
        assert_eq!(bayer.height, 2);
        assert_eq!(bayer.top_margin, 1);
        assert_eq!(bayer.left_margin, 1);
    }

    // Test demosaicing
    #[test]
    fn test_demosaic_output_size() {
        // 4x4 Bayer -> 4x4x3 RGB
        let data = vec![0.5f32; 16];
        let bayer = BayerImage::with_margins(&data, 4, 4, 4, 4, 0, 0, CfaPattern::Rggb);
        let rgb = demosaic_bilinear(&bayer);
        assert_eq!(rgb.len(), 4 * 4 * 3);
    }

    #[test]
    fn test_demosaic_with_margins() {
        // 6x6 raw with 1px margin -> 4x4 output
        let data = vec![0.5f32; 36];
        let bayer = BayerImage::with_margins(&data, 6, 6, 4, 4, 1, 1, CfaPattern::Rggb);
        let rgb = demosaic_bilinear(&bayer);
        assert_eq!(rgb.len(), 4 * 4 * 3);
    }

    #[test]
    fn test_demosaic_uniform_gray() {
        // Uniform gray input should produce uniform gray output
        let data = vec![0.5f32; 16];
        let bayer = BayerImage::with_margins(&data, 4, 4, 4, 4, 0, 0, CfaPattern::Rggb);
        let rgb = demosaic_bilinear(&bayer);

        for &v in &rgb {
            assert!((v - 0.5).abs() < 0.01, "Expected ~0.5, got {}", v);
        }
    }

    #[test]
    fn test_demosaic_preserves_red_at_red_pixel() {
        // Create a pattern where red pixels have value 1.0, others 0.0
        // RGGB pattern: (0,0) is red
        let mut data = vec![0.0f32; 16];
        // Set red pixels (0,0), (0,2), (2,0), (2,2) to 1.0
        data[0] = 1.0; // (0,0)
        data[2] = 1.0; // (0,2)
        data[8] = 1.0; // (2,0)
        data[10] = 1.0; // (2,2)

        let bayer = BayerImage::with_margins(&data, 4, 4, 4, 4, 0, 0, CfaPattern::Rggb);
        let rgb = demosaic_bilinear(&bayer);

        // At (0,0), R channel should be 1.0
        assert!((rgb[0] - 1.0).abs() < 0.01, "Red at (0,0) should be 1.0");
        // G and B at (0,0) should be 0.0 (interpolated from neighbors)
        assert!(rgb[1].abs() < 0.01, "Green at (0,0) should be ~0.0");
        assert!(rgb[2].abs() < 0.01, "Blue at (0,0) should be ~0.0");
    }

    #[test]
    fn test_demosaic_2x2_rggb() {
        // Minimal 2x2 RGGB pattern
        // R=1.0, G1=0.5, G2=0.5, B=0.0
        let data = vec![1.0, 0.5, 0.5, 0.0];
        let bayer = BayerImage::with_margins(&data, 2, 2, 2, 2, 0, 0, CfaPattern::Rggb);
        let rgb = demosaic_bilinear(&bayer);

        assert_eq!(rgb.len(), 12); // 2x2x3

        // (0,0) is R: R=1.0, G=interpolated, B=interpolated
        assert!((rgb[0] - 1.0).abs() < 0.01);

        // (0,1) is G: G=0.5
        assert!((rgb[4] - 0.5).abs() < 0.01);

        // (1,1) is B: B=0.0
        assert!((rgb[11] - 0.0).abs() < 0.01);
    }

    #[test]
    fn test_demosaic_edge_interpolation() {
        // Test that edge pixels don't panic and produce reasonable values
        let data = vec![0.5f32; 4]; // 2x2 minimum
        let bayer = BayerImage::with_margins(&data, 2, 2, 2, 2, 0, 0, CfaPattern::Rggb);
        let rgb = demosaic_bilinear(&bayer);

        // All values should be around 0.5 since input is uniform
        for &v in &rgb {
            assert!((0.0..=1.0).contains(&v), "Value {} out of range", v);
        }
    }

    #[test]
    fn test_demosaic_all_cfa_patterns() {
        // Test all CFA patterns produce valid output
        let data = vec![0.5f32; 16];

        for cfa in [
            CfaPattern::Rggb,
            CfaPattern::Bggr,
            CfaPattern::Grbg,
            CfaPattern::Gbrg,
        ] {
            let bayer = BayerImage::with_margins(&data, 4, 4, 4, 4, 0, 0, cfa);
            let rgb = demosaic_bilinear(&bayer);
            assert_eq!(rgb.len(), 48);
            for &v in &rgb {
                assert!(
                    (v - 0.5).abs() < 0.01,
                    "CFA {:?}: expected ~0.5, got {}",
                    cfa,
                    v
                );
            }
        }
    }

    #[test]
    fn test_interpolate_horizontal_edge_cases() {
        let data = vec![0.0, 1.0, 0.0, 1.0];
        let bayer = BayerImage::with_margins(&data, 2, 2, 2, 2, 0, 0, CfaPattern::Rggb);

        // At x=0, should use right neighbor twice
        let h0 = interpolate_horizontal(&bayer, 0, 0);
        assert!((h0 - 1.0).abs() < 0.01); // (data[1] + data[1]) / 2

        // At x=1, should use left neighbor twice
        let h1 = interpolate_horizontal(&bayer, 1, 0);
        assert!((h1 - 0.0).abs() < 0.01); // (data[0] + data[0]) / 2
    }

    #[test]
    fn test_interpolate_vertical_edge_cases() {
        let data = vec![0.0, 0.0, 1.0, 1.0];
        let bayer = BayerImage::with_margins(&data, 2, 2, 2, 2, 0, 0, CfaPattern::Rggb);

        // At y=0, should use bottom neighbor twice
        let v0 = interpolate_vertical(&bayer, 0, 0);
        assert!((v0 - 1.0).abs() < 0.01); // (data[2] + data[2]) / 2

        // At y=1, should use top neighbor twice
        let v1 = interpolate_vertical(&bayer, 0, 1);
        assert!((v1 - 0.0).abs() < 0.01); // (data[0] + data[0]) / 2
    }
}
