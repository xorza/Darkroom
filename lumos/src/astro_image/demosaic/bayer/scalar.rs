//! Scalar (non-SIMD) implementation of bilinear demosaicing.

use super::BayerImage;

/// Scalar implementation of bilinear demosaicing.
pub(crate) fn demosaic_bilinear_scalar(bayer: &BayerImage) -> Vec<f32> {
    let mut rgb = vec![0.0f32; bayer.width * bayer.height * 3];

    // Pre-compute pattern lookup for this CFA
    let pattern = bayer.cfa.pattern_2x2();

    for y in 0..bayer.height {
        // Map to raw coordinates
        let raw_y = y + bayer.top_margin;
        let red_in_row = bayer.cfa.red_in_row(raw_y);
        let row_pattern_idx = (raw_y & 1) << 1;

        for x in 0..bayer.width {
            let raw_x = x + bayer.left_margin;

            // Use pre-computed pattern instead of function call
            let color = pattern[row_pattern_idx | (raw_x & 1)];
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
                    if red_in_row {
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
#[inline(always)]
pub(crate) fn interpolate_horizontal(bayer: &BayerImage, x: usize, y: usize) -> f32 {
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
#[inline(always)]
pub(crate) fn interpolate_vertical(bayer: &BayerImage, x: usize, y: usize) -> f32 {
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

/// Interpolate from cross (4 neighbors) - optimized version.
#[inline(always)]
pub(crate) fn interpolate_cross(bayer: &BayerImage, x: usize, y: usize) -> f32 {
    let idx = y * bayer.raw_width + x;

    // Fast path for interior pixels (most common case)
    if x > 0 && x + 1 < bayer.raw_width && y > 0 && y + 1 < bayer.raw_height {
        let left = bayer.data[idx - 1];
        let right = bayer.data[idx + 1];
        let top = bayer.data[idx - bayer.raw_width];
        let bottom = bayer.data[idx + bayer.raw_width];
        return (left + right + top + bottom) * 0.25;
    }

    // Edge handling
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

/// Interpolate from diagonal neighbors - optimized version.
#[inline(always)]
pub(crate) fn interpolate_diagonal(bayer: &BayerImage, x: usize, y: usize) -> f32 {
    let idx = y * bayer.raw_width + x;

    // Fast path for interior pixels (most common case)
    if x > 0 && x + 1 < bayer.raw_width && y > 0 && y + 1 < bayer.raw_height {
        let tl = bayer.data[idx - bayer.raw_width - 1];
        let tr = bayer.data[idx - bayer.raw_width + 1];
        let bl = bayer.data[idx + bayer.raw_width - 1];
        let br = bayer.data[idx + bayer.raw_width + 1];
        return (tl + tr + bl + br) * 0.25;
    }

    // Edge handling
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
