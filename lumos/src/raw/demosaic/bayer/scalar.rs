//! Scalar (non-SIMD) implementation of bilinear demosaicing.

use super::BayerImage;

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
