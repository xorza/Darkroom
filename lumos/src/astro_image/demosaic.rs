/// Simple bilinear demosaicing of Bayer CFA data to RGB for libraw.
/// Handles raw image with margins - extracts the active area and demosaics.
/// Assumes RGGB Bayer pattern.
pub fn demosaic_bilinear_libraw(
    bayer: &[f32],
    raw_width: usize,
    raw_height: usize,
    width: usize,
    height: usize,
    top_margin: usize,
    left_margin: usize,
) -> Vec<f32> {
    let mut rgb = vec![0.0f32; width * height * 3];

    for y in 0..height {
        for x in 0..width {
            // Map to raw coordinates
            let raw_y = y + top_margin;
            let raw_x = x + left_margin;

            // Determine CFA color at this position (RGGB pattern)
            // (0,0)=R, (0,1)=G, (1,0)=G, (1,1)=B
            let color = match ((raw_y % 2), (raw_x % 2)) {
                (0, 0) => 0, // Red
                (0, 1) => 1, // Green (on red row)
                (1, 0) => 1, // Green (on blue row)
                (1, 1) => 2, // Blue
                _ => unreachable!(),
            };

            let rgb_idx = (y * width + x) * 3;

            // Get the value at this pixel in raw coordinates
            let val = bayer[raw_y * raw_width + raw_x];

            match color {
                0 => {
                    // Red pixel - interpolate G and B
                    rgb[rgb_idx] = val;
                    rgb[rgb_idx + 1] =
                        interpolate_cross_raw(bayer, raw_x, raw_y, raw_width, raw_height);
                    rgb[rgb_idx + 2] =
                        interpolate_diagonal_raw(bayer, raw_x, raw_y, raw_width, raw_height);
                }
                1 => {
                    // Green pixel - interpolate R and B
                    // On red row (raw_y % 2 == 0): R is horizontal, B is vertical
                    // On blue row (raw_y % 2 == 1): R is vertical, B is horizontal
                    if raw_y.is_multiple_of(2) {
                        rgb[rgb_idx] =
                            interpolate_horizontal_raw(bayer, raw_x, raw_y, raw_width, raw_height);
                        rgb[rgb_idx + 2] =
                            interpolate_vertical_raw(bayer, raw_x, raw_y, raw_width, raw_height);
                    } else {
                        rgb[rgb_idx] =
                            interpolate_vertical_raw(bayer, raw_x, raw_y, raw_width, raw_height);
                        rgb[rgb_idx + 2] =
                            interpolate_horizontal_raw(bayer, raw_x, raw_y, raw_width, raw_height);
                    }
                    rgb[rgb_idx + 1] = val;
                }
                2 => {
                    // Blue pixel - interpolate R and G
                    rgb[rgb_idx] =
                        interpolate_diagonal_raw(bayer, raw_x, raw_y, raw_width, raw_height);
                    rgb[rgb_idx + 1] =
                        interpolate_cross_raw(bayer, raw_x, raw_y, raw_width, raw_height);
                    rgb[rgb_idx + 2] = val;
                }
                _ => unreachable!(),
            }
        }
    }

    rgb
}

/// Interpolate from horizontal neighbors (raw coordinates).
#[inline]
fn interpolate_horizontal_raw(
    bayer: &[f32],
    x: usize,
    y: usize,
    width: usize,
    _height: usize,
) -> f32 {
    let idx = y * width + x;
    let left = if x > 0 {
        bayer[idx - 1]
    } else {
        bayer[idx + 1]
    };
    let right = if x + 1 < width {
        bayer[idx + 1]
    } else {
        bayer[idx - 1]
    };
    (left + right) * 0.5
}

/// Interpolate from vertical neighbors (raw coordinates).
#[inline]
fn interpolate_vertical_raw(bayer: &[f32], x: usize, y: usize, width: usize, height: usize) -> f32 {
    let idx = y * width + x;
    let top = if y > 0 {
        bayer[idx - width]
    } else {
        bayer[idx + width]
    };
    let bottom = if y + 1 < height {
        bayer[idx + width]
    } else {
        bayer[idx - width]
    };
    (top + bottom) * 0.5
}

/// Interpolate from cross (4 neighbors) (raw coordinates).
#[inline]
fn interpolate_cross_raw(bayer: &[f32], x: usize, y: usize, width: usize, height: usize) -> f32 {
    let idx = y * width + x;
    let mut sum = 0.0f32;
    let mut count = 0;

    if x > 0 {
        sum += bayer[idx - 1];
        count += 1;
    }
    if x + 1 < width {
        sum += bayer[idx + 1];
        count += 1;
    }
    if y > 0 {
        sum += bayer[idx - width];
        count += 1;
    }
    if y + 1 < height {
        sum += bayer[idx + width];
        count += 1;
    }

    if count > 0 {
        sum / count as f32
    } else {
        bayer[idx]
    }
}

/// Interpolate from diagonal neighbors (raw coordinates).
#[inline]
fn interpolate_diagonal_raw(bayer: &[f32], x: usize, y: usize, width: usize, height: usize) -> f32 {
    let idx = y * width + x;
    let mut sum = 0.0f32;
    let mut count = 0;

    if x > 0 && y > 0 {
        sum += bayer[idx - width - 1];
        count += 1;
    }
    if x + 1 < width && y > 0 {
        sum += bayer[idx - width + 1];
        count += 1;
    }
    if x > 0 && y + 1 < height {
        sum += bayer[idx + width - 1];
        count += 1;
    }
    if x + 1 < width && y + 1 < height {
        sum += bayer[idx + width + 1];
        count += 1;
    }

    if count > 0 {
        sum / count as f32
    } else {
        bayer[idx]
    }
}

/// Simple bilinear demosaicing of Bayer CFA data to RGB.
pub fn demosaic_bilinear(
    bayer: &[f32],
    width: usize,
    height: usize,
    cfa: &rawloader::CFA,
) -> Vec<f32> {
    let mut rgb = vec![0.0f32; width * height * 3];

    for y in 0..height {
        for x in 0..width {
            let color = cfa.color_at(y, x);
            let idx = y * width + x;
            let rgb_idx = idx * 3;

            // Get the value at this pixel
            let val = bayer[idx];

            // Determine what color this pixel represents
            // color: 0=R, 1=G, 2=B (typical mapping)
            match color {
                0 => {
                    // Red pixel - interpolate G and B
                    rgb[rgb_idx] = val;
                    rgb[rgb_idx + 1] = interpolate_cross(bayer, x, y, width, height);
                    rgb[rgb_idx + 2] = interpolate_diagonal(bayer, x, y, width, height);
                }
                1 => {
                    // Green pixel - interpolate R and B
                    // Need to figure out if R is on same row or column
                    let r_same_row = cfa.color_at(y, (x + 1).min(width - 1)) == 0
                        || (x > 0 && cfa.color_at(y, x - 1) == 0);
                    if r_same_row {
                        rgb[rgb_idx] = interpolate_horizontal(bayer, x, y, width, height);
                        rgb[rgb_idx + 2] = interpolate_vertical(bayer, x, y, width, height);
                    } else {
                        rgb[rgb_idx] = interpolate_vertical(bayer, x, y, width, height);
                        rgb[rgb_idx + 2] = interpolate_horizontal(bayer, x, y, width, height);
                    }
                    rgb[rgb_idx + 1] = val;
                }
                2 => {
                    // Blue pixel - interpolate R and G
                    rgb[rgb_idx] = interpolate_diagonal(bayer, x, y, width, height);
                    rgb[rgb_idx + 1] = interpolate_cross(bayer, x, y, width, height);
                    rgb[rgb_idx + 2] = val;
                }
                _ => {
                    // Unknown, just copy value to all channels
                    rgb[rgb_idx] = val;
                    rgb[rgb_idx + 1] = val;
                    rgb[rgb_idx + 2] = val;
                }
            }
        }
    }

    rgb
}

/// Interpolate from horizontal neighbors.
#[inline]
fn interpolate_horizontal(bayer: &[f32], x: usize, y: usize, width: usize, _height: usize) -> f32 {
    let idx = y * width + x;
    let left = if x > 0 {
        bayer[idx - 1]
    } else {
        bayer[idx + 1]
    };
    let right = if x + 1 < width {
        bayer[idx + 1]
    } else {
        bayer[idx - 1]
    };
    (left + right) * 0.5
}

/// Interpolate from vertical neighbors.
#[inline]
fn interpolate_vertical(bayer: &[f32], x: usize, y: usize, width: usize, height: usize) -> f32 {
    let idx = y * width + x;
    let top = if y > 0 {
        bayer[idx - width]
    } else {
        bayer[idx + width]
    };
    let bottom = if y + 1 < height {
        bayer[idx + width]
    } else {
        bayer[idx - width]
    };
    (top + bottom) * 0.5
}

/// Interpolate from cross (4 neighbors).
#[inline]
fn interpolate_cross(bayer: &[f32], x: usize, y: usize, width: usize, height: usize) -> f32 {
    let idx = y * width + x;
    let mut sum = 0.0f32;
    let mut count = 0;

    if x > 0 {
        sum += bayer[idx - 1];
        count += 1;
    }
    if x + 1 < width {
        sum += bayer[idx + 1];
        count += 1;
    }
    if y > 0 {
        sum += bayer[idx - width];
        count += 1;
    }
    if y + 1 < height {
        sum += bayer[idx + width];
        count += 1;
    }

    if count > 0 {
        sum / count as f32
    } else {
        bayer[idx]
    }
}

/// Interpolate from diagonal neighbors.
#[inline]
fn interpolate_diagonal(bayer: &[f32], x: usize, y: usize, width: usize, height: usize) -> f32 {
    let idx = y * width + x;
    let mut sum = 0.0f32;
    let mut count = 0;

    if x > 0 && y > 0 {
        sum += bayer[idx - width - 1];
        count += 1;
    }
    if x + 1 < width && y > 0 {
        sum += bayer[idx - width + 1];
        count += 1;
    }
    if x > 0 && y + 1 < height {
        sum += bayer[idx + width - 1];
        count += 1;
    }
    if x + 1 < width && y + 1 < height {
        sum += bayer[idx + width + 1];
        count += 1;
    }

    if count > 0 {
        sum / count as f32
    } else {
        bayer[idx]
    }
}
