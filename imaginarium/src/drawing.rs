//! Drawing primitives for images.
//!
//! Provides functions to draw shapes like circles, crosses, lines, and dots on images.
//! Works with f32 images (L_F32 or RGB_F32).

use crate::{Color, Image};
use glam::Vec2;

/// Draw a hollow circle on an image.
///
/// # Arguments
/// * `image` - The image to draw on (must be L_F32 or RGB_F32)
/// * `center` - Center coordinates
/// * `radius` - Circle radius in pixels
/// * `color` - Color (for grayscale images, uses luminance)
/// * `thickness` - Line thickness in pixels
pub fn draw_circle(image: &mut Image, center: Vec2, radius: f32, color: Color, thickness: f32) {
    let cx = center.x;
    let cy = center.y;
    let desc = *image.desc();
    let width = desc.width;
    let height = desc.height;
    let channels = desc.color_format.channel_count as usize;
    let stride = desc.stride / 4; // stride in f32 elements

    let pixels: &mut [f32] = bytemuck::cast_slice_mut(image.bytes_mut());

    let half_thick = thickness / 2.0;
    let min_radius = (radius - half_thick).max(0.0);
    let max_radius = radius + half_thick;
    let min_r_sq = min_radius * min_radius;
    let max_r_sq = max_radius * max_radius;

    // Bounding box for the circle
    let x_min = ((cx - max_radius).floor() as i32).max(0) as usize;
    let x_max = ((cx + max_radius).ceil() as i32).min(width as i32 - 1) as usize;
    let y_min = ((cy - max_radius).floor() as i32).max(0) as usize;
    let y_max = ((cy + max_radius).ceil() as i32).min(height as i32 - 1) as usize;

    for y in y_min..=y_max {
        for x in x_min..=x_max {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let dist_sq = dx * dx + dy * dy;

            if dist_sq >= min_r_sq && dist_sq <= max_r_sq {
                let idx = y * stride + x * channels;
                draw_pixel(pixels, idx, channels, color);
            }
        }
    }
}

/// Draw a filled circle (dot) on an image.
///
/// # Arguments
/// * `image` - The image to draw on (must be L_F32 or RGB_F32)
/// * `center` - Center coordinates
/// * `radius` - Circle radius in pixels
/// * `color` - Color (for grayscale images, uses luminance)
pub fn draw_dot(image: &mut Image, center: Vec2, radius: f32, color: Color) {
    let cx = center.x;
    let cy = center.y;
    let desc = *image.desc();
    let width = desc.width;
    let height = desc.height;
    let channels = desc.color_format.channel_count as usize;
    let stride = desc.stride / 4;

    let pixels: &mut [f32] = bytemuck::cast_slice_mut(image.bytes_mut());

    let r_sq = radius * radius;

    // Bounding box
    let x_min = ((cx - radius).floor() as i32).max(0) as usize;
    let x_max = ((cx + radius).ceil() as i32).min(width as i32 - 1) as usize;
    let y_min = ((cy - radius).floor() as i32).max(0) as usize;
    let y_max = ((cy + radius).ceil() as i32).min(height as i32 - 1) as usize;

    for y in y_min..=y_max {
        for x in x_min..=x_max {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let dist_sq = dx * dx + dy * dy;

            if dist_sq <= r_sq {
                let idx = y * stride + x * channels;
                draw_pixel(pixels, idx, channels, color);
            }
        }
    }
}

/// Draw a cross marker on an image.
///
/// # Arguments
/// * `image` - The image to draw on (must be L_F32 or RGB_F32)
/// * `center` - Center coordinates
/// * `arm_length` - Length of each arm from center
/// * `color` - Color (for grayscale images, uses luminance)
/// * `thickness` - Line thickness in pixels
pub fn draw_cross(image: &mut Image, center: Vec2, arm_length: f32, color: Color, thickness: f32) {
    let cx = center.x;
    let cy = center.y;
    // Horizontal arm
    draw_line(
        image,
        Vec2::new(cx - arm_length, cy),
        Vec2::new(cx + arm_length, cy),
        color,
        thickness,
    );
    // Vertical arm
    draw_line(
        image,
        Vec2::new(cx, cy - arm_length),
        Vec2::new(cx, cy + arm_length),
        color,
        thickness,
    );
}

/// Draw a line on an image using Bresenham-style algorithm with thickness.
///
/// # Arguments
/// * `image` - The image to draw on (must be L_F32 or RGB_F32)
/// * `start` - Start point
/// * `end` - End point
/// * `color` - Color (for grayscale images, uses luminance)
/// * `thickness` - Line thickness in pixels
pub fn draw_line(image: &mut Image, start: Vec2, end: Vec2, color: Color, thickness: f32) {
    let x1 = start.x;
    let y1 = start.y;
    let x2 = end.x;
    let y2 = end.y;
    let desc = *image.desc();
    let width = desc.width;
    let height = desc.height;
    let channels = desc.color_format.channel_count as usize;
    let stride = desc.stride / 4;

    let pixels: &mut [f32] = bytemuck::cast_slice_mut(image.bytes_mut());

    let dx = x2 - x1;
    let dy = y2 - y1;
    let length = (dx * dx + dy * dy).sqrt();

    if length < 0.001 {
        // Just a point
        let x = x1.round() as i32;
        let y = y1.round() as i32;
        if x >= 0 && x < width as i32 && y >= 0 && y < height as i32 {
            let idx = y as usize * stride + x as usize * channels;
            draw_pixel(pixels, idx, channels, color);
        }
        return;
    }

    // Normalized direction
    let ux = dx / length;
    let uy = dy / length;

    // Perpendicular direction for thickness
    let px = -uy;
    let py = ux;

    let half_thick = thickness / 2.0;

    // Step along the line
    let steps = (length.ceil() as usize).max(1);
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let lx = x1 + dx * t;
        let ly = y1 + dy * t;

        // Draw perpendicular pixels for thickness
        let thick_steps = (thickness.ceil() as i32).max(1);
        for j in -thick_steps..=thick_steps {
            let offset = j as f32 * 0.5;
            if offset.abs() > half_thick {
                continue;
            }

            let px_pos = lx + px * offset;
            let py_pos = ly + py * offset;

            let x = px_pos.round() as i32;
            let y = py_pos.round() as i32;

            if x >= 0 && x < width as i32 && y >= 0 && y < height as i32 {
                let idx = y as usize * stride + x as usize * channels;
                draw_pixel(pixels, idx, channels, color);
            }
        }
    }
}

/// Draw a rectangle outline on an image.
///
/// # Arguments
/// * `image` - The image to draw on (must be L_F32 or RGB_F32)
/// * `top_left` - Top-left corner coordinates
/// * `size` - Width and height as Vec2
/// * `color` - Color (for grayscale images, uses luminance)
/// * `thickness` - Line thickness in pixels
pub fn draw_rect(image: &mut Image, top_left: Vec2, size: Vec2, color: Color, thickness: f32) {
    let x = top_left.x;
    let y = top_left.y;
    let x2 = x + size.x;
    let y2 = y + size.y;

    // Top
    draw_line(image, Vec2::new(x, y), Vec2::new(x2, y), color, thickness);
    // Bottom
    draw_line(image, Vec2::new(x, y2), Vec2::new(x2, y2), color, thickness);
    // Left
    draw_line(image, Vec2::new(x, y), Vec2::new(x, y2), color, thickness);
    // Right
    draw_line(image, Vec2::new(x2, y), Vec2::new(x2, y2), color, thickness);
}

/// Helper to draw a single pixel with the given color.
#[inline]
fn draw_pixel(pixels: &mut [f32], idx: usize, channels: usize, color: Color) {
    if channels == 1 {
        // Grayscale: use luminance
        pixels[idx] = color.luminance();
    } else if channels >= 3 {
        pixels[idx] = color.r;
        pixels[idx + 1] = color.g;
        pixels[idx + 2] = color.b;
        if channels == 4 {
            pixels[idx + 3] = color.a;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ColorFormat, ImageDesc};

    fn create_test_image(width: usize, height: usize, channels: usize) -> Image {
        let format = if channels == 1 {
            ColorFormat::L_F32
        } else {
            ColorFormat::RGB_F32
        };
        let desc = ImageDesc::new_packed(width, height, format);
        Image::new_black(desc).unwrap()
    }

    #[test]
    fn test_draw_circle() {
        let mut img = create_test_image(100, 100, 3);
        draw_circle(&mut img, Vec2::new(50.0, 50.0), 20.0, Color::RED, 2.0);

        // Check that some pixels were drawn
        let pixels: &[f32] = bytemuck::cast_slice(img.bytes());
        let has_red = pixels
            .chunks(3)
            .any(|p| p[0] > 0.5 && p[1] < 0.1 && p[2] < 0.1);
        assert!(has_red, "Circle should have red pixels");
    }

    #[test]
    fn test_draw_dot() {
        let mut img = create_test_image(100, 100, 3);
        draw_dot(&mut img, Vec2::new(50.0, 50.0), 5.0, Color::GREEN);

        // Check center pixel
        let pixels: &[f32] = bytemuck::cast_slice(img.bytes());
        let idx = (50 * 100 + 50) * 3;
        assert!(pixels[idx + 1] > 0.5, "Center should be green");
    }

    #[test]
    fn test_draw_cross() {
        let mut img = create_test_image(100, 100, 3);
        draw_cross(&mut img, Vec2::new(50.0, 50.0), 10.0, Color::BLUE, 1.0);

        // Check center pixel
        let pixels: &[f32] = bytemuck::cast_slice(img.bytes());
        let idx = (50 * 100 + 50) * 3;
        assert!(pixels[idx + 2] > 0.5, "Center should be blue");
    }

    #[test]
    fn test_draw_line() {
        let mut img = create_test_image(100, 100, 3);
        draw_line(
            &mut img,
            Vec2::new(10.0, 10.0),
            Vec2::new(90.0, 90.0),
            Color::WHITE,
            1.0,
        );

        // Check a point on the diagonal
        let pixels: &[f32] = bytemuck::cast_slice(img.bytes());
        let idx = (50 * 100 + 50) * 3;
        assert!(pixels[idx] > 0.5, "Diagonal point should be white");
    }

    #[test]
    fn test_draw_rect() {
        let mut img = create_test_image(100, 100, 3);
        draw_rect(
            &mut img,
            Vec2::new(20.0, 20.0),
            Vec2::new(60.0, 40.0),
            Color::YELLOW,
            1.0,
        );

        // Check a corner pixel
        let pixels: &[f32] = bytemuck::cast_slice(img.bytes());
        let idx = (20 * 100 + 20) * 3;
        assert!(
            pixels[idx] > 0.5 && pixels[idx + 1] > 0.5,
            "Corner should be yellow"
        );
    }

    #[test]
    fn test_draw_on_grayscale() {
        let mut img = create_test_image(100, 100, 1);
        draw_circle(&mut img, Vec2::new(50.0, 50.0), 10.0, Color::WHITE, 2.0);

        // Check that pixels were drawn
        let pixels: &[f32] = bytemuck::cast_slice(img.bytes());
        let has_bright = pixels.iter().any(|&p| p > 0.5);
        assert!(has_bright, "Should have bright pixels");
    }
}
