//! Scalar implementation of X-Trans bilinear demosaicing.
//!
//! This is a basic bilinear interpolation for X-Trans sensors.
//! It's simpler and faster than advanced algorithms like Markesteijn,
//! but produces lower quality results (may show artifacts).

use rayon::prelude::*;

use super::XTransImage;

/// Minimum image size to use parallel processing (avoids overhead for small images).
const MIN_PARALLEL_SIZE: usize = 128;

/// Bilinear demosaicing for X-Trans CFA.
///
/// For each pixel, interpolates missing color channels from nearby pixels
/// of the same color. This is a simple approach that works but may produce
/// artifacts in fine detail areas.
///
/// Uses rayon for parallel row processing on large images to avoid false
/// cache sharing (each thread writes to separate cache lines).
///
/// Returns RGB interleaved data: [R0, G0, B0, R1, G1, B1, ...]
pub fn demosaic_xtrans_bilinear(xtrans: &XTransImage) -> Vec<f32> {
    let use_parallel = xtrans.width >= MIN_PARALLEL_SIZE && xtrans.height >= MIN_PARALLEL_SIZE;

    if use_parallel {
        demosaic_parallel(xtrans)
    } else {
        demosaic_scalar(xtrans)
    }
}

/// Parallel row-based demosaicing.
/// Processes rows in parallel using rayon, with each thread writing to its own
/// row buffer to avoid false cache sharing.
fn demosaic_parallel(xtrans: &XTransImage) -> Vec<f32> {
    let mut rgb = vec![0.0f32; xtrans.width * xtrans.height * 3];

    // Process rows in parallel - each row is a separate chunk
    // This ensures no false sharing since each thread writes to different cache lines
    let row_stride = xtrans.width * 3;
    rgb.par_chunks_mut(row_stride)
        .enumerate()
        .for_each(|(y, row_rgb)| {
            process_row(xtrans, y, row_rgb);
        });

    rgb
}

/// Sequential scalar demosaicing for small images.
fn demosaic_scalar(xtrans: &XTransImage) -> Vec<f32> {
    let mut rgb = vec![0.0f32; xtrans.width * xtrans.height * 3];

    for y in 0..xtrans.height {
        let row_start = y * xtrans.width * 3;
        let row_rgb = &mut rgb[row_start..row_start + xtrans.width * 3];
        process_row(xtrans, y, row_rgb);
    }

    rgb
}

/// Process a single row of the image.
#[inline]
fn process_row(xtrans: &XTransImage, y: usize, row_rgb: &mut [f32]) {
    let raw_y = y + xtrans.top_margin;

    for x in 0..xtrans.width {
        let raw_x = x + xtrans.left_margin;
        let rgb_idx = x * 3;

        let color = xtrans.pattern.color_at(raw_y, raw_x);
        let val = xtrans.data[raw_y * xtrans.raw_width + raw_x];

        // Set the known color channel
        row_rgb[rgb_idx + color as usize] = val;

        // Interpolate the other two channels
        for c in 0u8..3 {
            if c != color {
                row_rgb[rgb_idx + c as usize] = interpolate_channel(xtrans, raw_x, raw_y, c);
            }
        }
    }
}

/// Interpolate a specific color channel at position (x, y).
/// Searches nearby pixels for the same color and averages them.
#[inline]
fn interpolate_channel(xtrans: &XTransImage, x: usize, y: usize, target_color: u8) -> f32 {
    // Search in a 5x5 neighborhood for pixels of the target color
    // This is sufficient for X-Trans since within any 6x6 block,
    // each color appears multiple times
    let mut sum = 0.0f32;
    let mut count = 0u32;

    // Search radius of 2 pixels (5x5 neighborhood)
    for dy in -2i32..=2 {
        for dx in -2i32..=2 {
            let ny = y as i32 + dy;
            let nx = x as i32 + dx;

            // Bounds check
            if ny < 0 || nx < 0 || ny >= xtrans.raw_height as i32 || nx >= xtrans.raw_width as i32 {
                continue;
            }

            let ny = ny as usize;
            let nx = nx as usize;

            // Check if this pixel has the target color
            if xtrans.pattern.color_at(ny, nx) == target_color {
                sum += xtrans.data[ny * xtrans.raw_width + nx];
                count += 1;
            }
        }
    }

    if count > 0 {
        sum / count as f32
    } else {
        // Fallback: shouldn't happen with valid X-Trans pattern
        // but return 0 if no matching pixels found
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::super::XTransPattern;
    use super::*;

    // Standard X-Trans pattern
    fn test_pattern() -> XTransPattern {
        XTransPattern::new([
            [1, 0, 1, 1, 2, 1], // G R G G B G
            [2, 1, 2, 0, 1, 0], // B G B R G R
            [1, 2, 1, 1, 0, 1], // G B G G R G
            [1, 2, 1, 1, 0, 1], // G B G G R G
            [0, 1, 0, 2, 1, 2], // R G R B G B
            [1, 0, 1, 1, 2, 1], // G R G G B G
        ])
    }

    #[test]
    fn test_demosaic_output_size() {
        // 12x12 gives us 2 complete X-Trans patterns
        let data = vec![0.5f32; 12 * 12];
        let pattern = test_pattern();
        let xtrans = XTransImage::with_margins(&data, 12, 12, 12, 12, 0, 0, pattern);

        let rgb = demosaic_xtrans_bilinear(&xtrans);
        assert_eq!(rgb.len(), 12 * 12 * 3);
    }

    #[test]
    fn test_demosaic_uniform_gray() {
        // Uniform input should produce roughly uniform output
        let data = vec![0.5f32; 12 * 12];
        let pattern = test_pattern();
        let xtrans = XTransImage::with_margins(&data, 12, 12, 12, 12, 0, 0, pattern);

        let rgb = demosaic_xtrans_bilinear(&xtrans);

        for &v in &rgb {
            assert!((v - 0.5).abs() < 0.01, "Expected ~0.5, got {}", v);
        }
    }

    #[test]
    fn test_demosaic_preserves_known_channel() {
        // At a known pixel position, the original value should be preserved
        let mut data = vec![0.0f32; 12 * 12];
        // Set position (0, 1) which is Red in our test pattern
        data[1] = 1.0;

        let pattern = test_pattern();
        let xtrans = XTransImage::with_margins(&data, 12, 12, 12, 12, 0, 0, pattern);

        let rgb = demosaic_xtrans_bilinear(&xtrans);

        // Position (0, 1) -> RGB index 1*3 = 3, Red channel is +0
        let red_at_0_1 = rgb[3];
        assert!(
            (red_at_0_1 - 1.0).abs() < 0.001,
            "Red channel at (0,1) should be 1.0, got {}",
            red_at_0_1
        );
    }

    #[test]
    fn test_demosaic_with_margins() {
        // Test that margins are handled correctly
        let data = vec![0.5f32; 18 * 18];
        let pattern = test_pattern();
        let xtrans = XTransImage::with_margins(&data, 18, 18, 12, 12, 3, 3, pattern);

        let rgb = demosaic_xtrans_bilinear(&xtrans);
        assert_eq!(rgb.len(), 12 * 12 * 3);

        // All values should be approximately 0.5
        for &v in &rgb {
            assert!((v - 0.5).abs() < 0.01, "Expected ~0.5, got {}", v);
        }
    }

    #[test]
    fn test_interpolate_channel_finds_neighbors() {
        let data = vec![0.5f32; 12 * 12];
        let pattern = test_pattern();
        let xtrans = XTransImage::with_margins(&data, 12, 12, 12, 12, 0, 0, pattern);

        // Test interpolation at center of image
        let val = interpolate_channel(&xtrans, 6, 6, 0); // Red
        assert!(val > 0.0, "Should find red neighbors");

        let val = interpolate_channel(&xtrans, 6, 6, 1); // Green
        assert!(val > 0.0, "Should find green neighbors");

        let val = interpolate_channel(&xtrans, 6, 6, 2); // Blue
        assert!(val > 0.0, "Should find blue neighbors");
    }
}
