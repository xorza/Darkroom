//! Image writing utilities for visual tests.

use image::{GrayImage, Rgb, RgbImage};
use std::path::Path;

/// Convert f32 pixels to grayscale image (clamped to 0-1).
pub fn to_gray_image(pixels: &[f32], width: usize, height: usize) -> GrayImage {
    let bytes: Vec<u8> = pixels
        .iter()
        .map(|&p| (p.clamp(0.0, 1.0) * 255.0) as u8)
        .collect();
    GrayImage::from_raw(width as u32, height as u32, bytes).unwrap()
}

/// Convert f32 pixels to grayscale image with auto-stretching.
pub fn to_gray_stretched(pixels: &[f32], width: usize, height: usize) -> GrayImage {
    let min = pixels.iter().cloned().fold(f32::INFINITY, f32::min);
    let max = pixels.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let range = (max - min).max(1e-10);

    let bytes: Vec<u8> = pixels
        .iter()
        .map(|&p| (((p - min) / range) * 255.0) as u8)
        .collect();

    GrayImage::from_raw(width as u32, height as u32, bytes).unwrap()
}

/// Convert boolean mask to grayscale image.
pub fn mask_to_gray(mask: &[bool], width: usize, height: usize) -> GrayImage {
    let bytes: Vec<u8> = mask.iter().map(|&b| if b { 255 } else { 0 }).collect();
    GrayImage::from_raw(width as u32, height as u32, bytes).unwrap()
}

/// Convert labeled image to colored visualization.
///
/// Each label gets a unique color for easy visualization.
pub fn labels_to_rgb(labels: &[u32], width: usize, height: usize) -> RgbImage {
    // Generate distinct colors for labels using golden ratio
    let label_to_color = |label: u32| -> Rgb<u8> {
        if label == 0 {
            return Rgb([0, 0, 0]);
        }
        let hue = ((label as f32) * 0.618_034) % 1.0;
        hsv_to_rgb(hue, 0.8, 0.9)
    };

    let pixels: Vec<u8> = labels
        .iter()
        .flat_map(|&l| {
            let Rgb([r, g, b]) = label_to_color(l);
            [r, g, b]
        })
        .collect();

    RgbImage::from_raw(width as u32, height as u32, pixels).unwrap()
}

/// Convert HSV to RGB color.
fn hsv_to_rgb(h: f32, s: f32, v: f32) -> Rgb<u8> {
    let c = v * s;
    let x = c * (1.0 - ((h * 6.0) % 2.0 - 1.0).abs());
    let m = v - c;

    let (r, g, b) = match (h * 6.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };

    Rgb([
        ((r + m) * 255.0) as u8,
        ((g + m) * 255.0) as u8,
        ((b + m) * 255.0) as u8,
    ])
}

/// Convert grayscale f32 pixels to RGB image.
pub fn gray_to_rgb(pixels: &[f32], width: usize, height: usize) -> RgbImage {
    let data: Vec<u8> = pixels
        .iter()
        .flat_map(|&p| {
            let v = (p.clamp(0.0, 1.0) * 255.0) as u8;
            [v, v, v]
        })
        .collect();

    RgbImage::from_raw(width as u32, height as u32, data).unwrap()
}

/// Convert grayscale f32 pixels to RGB image with auto-stretching.
pub fn gray_to_rgb_stretched(pixels: &[f32], width: usize, height: usize) -> RgbImage {
    let min = pixels.iter().cloned().fold(f32::INFINITY, f32::min);
    let max = pixels.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let range = (max - min).max(1e-10);

    let data: Vec<u8> = pixels
        .iter()
        .flat_map(|&p| {
            let v = (((p - min) / range) * 255.0) as u8;
            [v, v, v]
        })
        .collect();

    RgbImage::from_raw(width as u32, height as u32, data).unwrap()
}

/// Save grayscale image to PNG file.
pub fn save_grayscale_png(pixels: &[f32], width: usize, height: usize, path: &Path) {
    let img = to_gray_image(pixels, width, height);
    img.save(path).expect("Failed to save grayscale PNG");
}

/// Save grayscale image with auto-stretch to PNG file.
pub fn save_grayscale_stretched_png(pixels: &[f32], width: usize, height: usize, path: &Path) {
    let img = to_gray_stretched(pixels, width, height);
    img.save(path)
        .expect("Failed to save stretched grayscale PNG");
}

/// Save RGB image to PNG file.
pub fn save_rgb_png(image: &RgbImage, path: &Path) {
    image.save(path).expect("Failed to save RGB PNG");
}

/// Save comparison image showing ground truth vs detected stars.
pub fn save_comparison_png(
    pixels: &[f32],
    width: usize,
    height: usize,
    ground_truth: &[crate::star_detection::visual_tests::generators::GroundTruthStar],
    detected: &[crate::star_detection::Star],
    match_radius: f32,
    path: &Path,
) {
    let image = super::comparison::create_comparison_image(
        pixels,
        width,
        height,
        ground_truth,
        detected,
        match_radius,
    );
    image.save(path).expect("Failed to save comparison PNG");
}

/// Save mask to PNG file.
pub fn save_mask_png(mask: &[bool], width: usize, height: usize, path: &Path) {
    let img = mask_to_gray(mask, width, height);
    img.save(path).expect("Failed to save mask PNG");
}

/// Create a side-by-side comparison image.
pub fn side_by_side(left: &GrayImage, right: &GrayImage) -> GrayImage {
    let w1 = left.width();
    let w2 = right.width();
    let h = left.height().max(right.height());
    let gap = 4;

    let mut result = GrayImage::new(w1 + w2 + gap, h);

    // Copy left image
    for y in 0..left.height() {
        for x in 0..w1 {
            result.put_pixel(x, y, *left.get_pixel(x, y));
        }
    }

    // Add gap (white line)
    for y in 0..h {
        for x in 0..gap {
            result.put_pixel(w1 + x, y, image::Luma([128]));
        }
    }

    // Copy right image
    for y in 0..right.height() {
        for x in 0..w2 {
            result.put_pixel(w1 + gap + x, y, *right.get_pixel(x, y));
        }
    }

    result
}

/// Create a side-by-side RGB comparison image.
pub fn side_by_side_rgb(left: &RgbImage, right: &RgbImage) -> RgbImage {
    let w1 = left.width();
    let w2 = right.width();
    let h = left.height().max(right.height());
    let gap = 4;

    let mut result = RgbImage::new(w1 + w2 + gap, h);

    // Copy left image
    for y in 0..left.height() {
        for x in 0..w1 {
            result.put_pixel(x, y, *left.get_pixel(x, y));
        }
    }

    // Add gap (gray line)
    for y in 0..h {
        for x in 0..gap {
            result.put_pixel(w1 + x, y, Rgb([128, 128, 128]));
        }
    }

    // Copy right image
    for y in 0..right.height() {
        for x in 0..w2 {
            result.put_pixel(w1 + gap + x, y, *right.get_pixel(x, y));
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gray_image_conversion() {
        let pixels = vec![0.0, 0.5, 1.0, 0.25];
        let img = to_gray_image(&pixels, 2, 2);

        assert_eq!(img.get_pixel(0, 0).0[0], 0);
        assert_eq!(img.get_pixel(1, 0).0[0], 127);
        assert_eq!(img.get_pixel(0, 1).0[0], 255);
        assert_eq!(img.get_pixel(1, 1).0[0], 63);
    }

    #[test]
    fn test_stretched_conversion() {
        let pixels = vec![0.2, 0.4, 0.6, 0.8];
        let img = to_gray_stretched(&pixels, 2, 2);

        // Should stretch 0.2-0.8 to 0-255
        assert_eq!(img.get_pixel(0, 0).0[0], 0); // 0.2 -> 0
        assert_eq!(img.get_pixel(1, 1).0[0], 255); // 0.8 -> 255
    }
}
