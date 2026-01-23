/// Bayer CFA (Color Filter Array) pattern.
/// Represents the 2x2 pattern of color filters on the sensor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

impl From<&rawloader::CFA> for CfaPattern {
    fn from(cfa: &rawloader::CFA) -> Self {
        // Sample the 2x2 pattern to determine the CFA type
        let c00 = cfa.color_at(0, 0);
        let c01 = cfa.color_at(0, 1);

        match (c00, c01) {
            (0, 1) => CfaPattern::Rggb, // R G
            (2, 1) => CfaPattern::Bggr, // B G
            (1, 0) => CfaPattern::Grbg, // G R
            (1, 2) => CfaPattern::Gbrg, // G B
            _ => CfaPattern::Rggb,      // Default fallback
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
    /// Create a BayerImage without margins (raw size == output size).
    pub fn new(data: &'a [f32], width: usize, height: usize, cfa: CfaPattern) -> Self {
        Self {
            data,
            raw_width: width,
            raw_height: height,
            width,
            height,
            top_margin: 0,
            left_margin: 0,
            cfa,
        }
    }

    /// Create a BayerImage with margins (libraw style).
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
