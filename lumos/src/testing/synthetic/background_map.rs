//! ImageStats generation for testing.
//!
//! Provides utilities to create ImageStats instances for benchmarks and tests.

use crate::common::Buffer2;
use crate::star_detection::image_stats::ImageStats;

/// Create a uniform ImageStats with constant background and noise values.
pub fn uniform(width: usize, height: usize, background: f32, noise: f32) -> ImageStats {
    let mut bg_buf = Buffer2::new_default(width, height);
    let mut noise_buf = Buffer2::new_default(width, height);
    bg_buf.fill(background);
    noise_buf.fill(noise);
    ImageStats {
        background: bg_buf,
        noise: noise_buf,
        adaptive_sigma: None,
    }
}

/// Create an ImageStats with a horizontal gradient in the background.
pub fn horizontal_gradient(
    width: usize,
    height: usize,
    bg_left: f32,
    bg_right: f32,
    noise: f32,
) -> ImageStats {
    let mut bg_buf = Buffer2::new_default(width, height);
    let mut noise_buf = Buffer2::new_default(width, height);
    for y in 0..height {
        for x in 0..width {
            let t = if width > 1 {
                x as f32 / (width - 1) as f32
            } else {
                0.5
            };
            bg_buf[(x, y)] = bg_left + t * (bg_right - bg_left);
        }
    }
    noise_buf.fill(noise);
    ImageStats {
        background: bg_buf,
        noise: noise_buf,
        adaptive_sigma: None,
    }
}

/// Create an ImageStats with a vertical gradient in the background.
pub fn vertical_gradient(
    width: usize,
    height: usize,
    bg_top: f32,
    bg_bottom: f32,
    noise: f32,
) -> ImageStats {
    let mut bg_buf = Buffer2::new_default(width, height);
    let mut noise_buf = Buffer2::new_default(width, height);
    for y in 0..height {
        let t = if height > 1 {
            y as f32 / (height - 1) as f32
        } else {
            0.5
        };
        let value = bg_top + t * (bg_bottom - bg_top);
        for x in 0..width {
            bg_buf[(x, y)] = value;
        }
    }
    noise_buf.fill(noise);
    ImageStats {
        background: bg_buf,
        noise: noise_buf,
        adaptive_sigma: None,
    }
}

/// Create an ImageStats with radial vignette in the background.
pub fn vignette(
    width: usize,
    height: usize,
    bg_center: f32,
    bg_edge: f32,
    noise: f32,
) -> ImageStats {
    let mut bg_buf = Buffer2::new_default(width, height);
    let mut noise_buf = Buffer2::new_default(width, height);
    let cx = width as f32 / 2.0;
    let cy = height as f32 / 2.0;
    let max_r = (cx * cx + cy * cy).sqrt();

    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let r = (dx * dx + dy * dy).sqrt();
            let t = if max_r > 0.0 { r / max_r } else { 0.0 };
            bg_buf[(x, y)] = bg_center + t * (bg_edge - bg_center);
        }
    }
    noise_buf.fill(noise);
    ImageStats {
        background: bg_buf,
        noise: noise_buf,
        adaptive_sigma: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_uniform() {
        let bg = uniform(100, 100, 0.1, 0.01);
        assert_eq!(bg.background.width(), 100);
        assert_eq!(bg.background.height(), 100);
        assert!((bg.background[(50, 50)] - 0.1).abs() < 1e-6);
        assert!((bg.noise[(50, 50)] - 0.01).abs() < 1e-6);
    }

    #[test]
    fn test_horizontal_gradient() {
        let bg = horizontal_gradient(100, 10, 0.0, 1.0, 0.05);
        assert!((bg.background[(0, 5)] - 0.0).abs() < 1e-6);
        assert!((bg.background[(99, 5)] - 1.0).abs() < 1e-6);
        assert!((bg.noise[(50, 5)] - 0.05).abs() < 1e-6);
    }

    #[test]
    fn test_vertical_gradient() {
        let bg = vertical_gradient(10, 100, 0.0, 1.0, 0.02);
        assert!((bg.background[(5, 0)] - 0.0).abs() < 1e-6);
        assert!((bg.background[(5, 99)] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_vignette() {
        let bg = vignette(100, 100, 1.0, 0.5, 0.01);
        // Center should be brighter
        assert!(bg.background[(50, 50)] > bg.background[(0, 0)]);
    }
}
