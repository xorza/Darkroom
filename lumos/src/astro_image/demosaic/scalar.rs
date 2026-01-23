//! Scalar (non-SIMD) implementation of bilinear demosaicing.

use super::BayerImage;

/// Scalar implementation of bilinear demosaicing.
pub(crate) fn demosaic_bilinear_scalar(bayer: &BayerImage) -> Vec<f32> {
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

/// Process a single pixel with scalar code (used for borders and fallback in SIMD).
#[inline]
pub(crate) fn process_pixel_scalar(
    bayer: &BayerImage,
    rgb: &mut [f32],
    x: usize,
    y: usize,
    raw_x: usize,
    raw_y: usize,
) {
    let color = bayer.cfa.color_at(raw_y, raw_x);
    let rgb_idx = (y * bayer.width + x) * 3;
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

/// Interpolate from horizontal neighbors.
#[inline]
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
#[inline]
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
