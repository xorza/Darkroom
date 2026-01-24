//! Visual tests for star detection - generates debug images for inspection.

use crate::AstroImage;
use crate::star_detection::{Star, StarDetectionConfig, find_stars};
use crate::testing::{calibration_dir, init_tracing};

/// Draw a circle on an RGB u8 image (interleaved format with stride).
#[allow(clippy::too_many_arguments)]
fn draw_circle_rgb8(
    pixels: &mut [u8],
    width: usize,
    height: usize,
    stride: usize,
    cx: f32,
    cy: f32,
    radius: f32,
    color: [u8; 3],
) {
    let r2 = radius * radius;
    let inner_r2 = (radius - 1.5).max(0.0).powi(2);

    let x_min = (cx - radius - 1.0).max(0.0) as usize;
    let x_max = ((cx + radius + 1.0) as usize).min(width - 1);
    let y_min = (cy - radius - 1.0).max(0.0) as usize;
    let y_max = ((cy + radius + 1.0) as usize).min(height - 1);

    for y in y_min..=y_max {
        for x in x_min..=x_max {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let dist2 = dx * dx + dy * dy;

            // Draw ring (between inner and outer radius)
            if dist2 <= r2 && dist2 >= inner_r2 {
                let idx = y * stride + x * 3;
                pixels[idx] = color[0];
                pixels[idx + 1] = color[1];
                pixels[idx + 2] = color[2];
            }
        }
    }
}

/// Draw a cross/plus marker on an RGB u8 image.
#[allow(clippy::too_many_arguments)]
fn draw_cross_rgb8(
    pixels: &mut [u8],
    width: usize,
    height: usize,
    stride: usize,
    cx: f32,
    cy: f32,
    size: f32,
    color: [u8; 3],
) {
    let half = size / 2.0;

    // Horizontal line
    let y = cy.round() as isize;
    if y >= 0 && y < height as isize {
        let x_min = (cx - half).max(0.0) as usize;
        let x_max = ((cx + half) as usize).min(width - 1);
        for x in x_min..=x_max {
            let idx = y as usize * stride + x * 3;
            pixels[idx] = color[0];
            pixels[idx + 1] = color[1];
            pixels[idx + 2] = color[2];
        }
    }

    // Vertical line
    let x = cx.round() as isize;
    if x >= 0 && x < width as isize {
        let y_min = (cy - half).max(0.0) as usize;
        let y_max = ((cy + half) as usize).min(height - 1);
        for y in y_min..=y_max {
            let idx = y * stride + x as usize * 3;
            pixels[idx] = color[0];
            pixels[idx + 1] = color[1];
            pixels[idx + 2] = color[2];
        }
    }
}

/// Draw text label near a star (simple digit rendering).
#[allow(clippy::too_many_arguments)]
fn draw_label_rgb8(
    pixels: &mut [u8],
    width: usize,
    height: usize,
    stride: usize,
    x: f32,
    y: f32,
    label: usize,
    color: [u8; 3],
) {
    // Simple 3x5 digit font
    const DIGITS: [[u8; 15]; 10] = [
        [1, 1, 1, 1, 0, 1, 1, 0, 1, 1, 0, 1, 1, 1, 1], // 0
        [0, 1, 0, 1, 1, 0, 0, 1, 0, 0, 1, 0, 1, 1, 1], // 1
        [1, 1, 1, 0, 0, 1, 1, 1, 1, 1, 0, 0, 1, 1, 1], // 2
        [1, 1, 1, 0, 0, 1, 1, 1, 1, 0, 0, 1, 1, 1, 1], // 3
        [1, 0, 1, 1, 0, 1, 1, 1, 1, 0, 0, 1, 0, 0, 1], // 4
        [1, 1, 1, 1, 0, 0, 1, 1, 1, 0, 0, 1, 1, 1, 1], // 5
        [1, 1, 1, 1, 0, 0, 1, 1, 1, 1, 0, 1, 1, 1, 1], // 6
        [1, 1, 1, 0, 0, 1, 0, 0, 1, 0, 0, 1, 0, 0, 1], // 7
        [1, 1, 1, 1, 0, 1, 1, 1, 1, 1, 0, 1, 1, 1, 1], // 8
        [1, 1, 1, 1, 0, 1, 1, 1, 1, 0, 0, 1, 1, 1, 1], // 9
    ];

    let digits: Vec<usize> = if label == 0 {
        vec![0]
    } else {
        let mut n = label;
        let mut d = Vec::new();
        while n > 0 {
            d.push(n % 10);
            n /= 10;
        }
        d.reverse();
        d
    };

    let start_x = (x + 10.0) as isize;
    let start_y = (y - 2.0) as isize;

    for (digit_idx, &digit) in digits.iter().enumerate() {
        let offset_x = start_x + (digit_idx as isize * 4);
        for row in 0..5 {
            for col in 0..3 {
                if DIGITS[digit][row * 3 + col] == 1 {
                    let px = offset_x + col as isize;
                    let py = start_y + row as isize;
                    if px >= 0 && px < width as isize && py >= 0 && py < height as isize {
                        let idx = py as usize * stride + px as usize * 3;
                        pixels[idx] = color[0];
                        pixels[idx + 1] = color[1];
                        pixels[idx + 2] = color[2];
                    }
                }
            }
        }
    }
}

#[test]
#[cfg_attr(not(feature = "slow-tests"), ignore)]
fn test_visualize_star_detection() {
    init_tracing();

    let Some(cal_dir) = calibration_dir() else {
        eprintln!("LUMOS_CALIBRATION_DIR not set, skipping test");
        return;
    };

    let lights_dir = cal_dir.join("Lights");
    if !lights_dir.exists() {
        eprintln!("Lights directory not found, skipping test");
        return;
    }

    let files = common::file_utils::astro_image_files(&lights_dir);
    let Some(first_file) = files.first() else {
        eprintln!("No image files in Lights, skipping test");
        return;
    };

    println!("Loading light frame: {:?}", first_file);
    let image = AstroImage::from_file(first_file).expect("Failed to load image");
    let (width, height) = (image.dimensions.width, image.dimensions.height);
    println!("Image size: {}x{}", width, height);

    // Convert to grayscale for star detection
    let grayscale: Vec<f32> = if image.dimensions.channels == 3 {
        (0..width * height)
            .map(|i| {
                let r = image.pixels[i];
                let g = image.pixels[width * height + i];
                let b = image.pixels[2 * width * height + i];
                0.2126 * r + 0.7152 * g + 0.0722 * b
            })
            .collect()
    } else {
        image.pixels.clone()
    };

    // Find stars
    let config = StarDetectionConfig::default();
    let stars = find_stars(&grayscale, width, height, &config);
    println!("Found {} stars", stars.len());

    // Convert AstroImage to imaginarium::Image, then to RGB_U8
    let imag_image: imaginarium::Image = image.into();
    let mut imag_image = imag_image
        .convert(imaginarium::ColorFormat::RGB_U8)
        .expect("Failed to convert to RGB_U8");

    let stride = imag_image.desc().stride;
    let output_pixels = imag_image.bytes_mut();

    // Draw only top N best stars (by SNR)
    let max_stars = 200;
    let top_stars: Vec<&Star> = stars.iter().take(max_stars).collect();

    println!("Drawing top {} stars", top_stars.len());

    // Green color for markers
    let color = [0u8, 255, 0];

    for (idx, star) in top_stars.iter().enumerate() {
        let radius = (star.fwhm * 1.5).max(8.0);

        // Draw circle
        draw_circle_rgb8(
            output_pixels,
            width,
            height,
            stride,
            star.x,
            star.y,
            radius,
            color,
        );

        // Draw cross at centroid
        draw_cross_rgb8(
            output_pixels,
            width,
            height,
            stride,
            star.x,
            star.y,
            7.0,
            color,
        );

        // Draw index label
        draw_label_rgb8(
            output_pixels,
            width,
            height,
            stride,
            star.x,
            star.y,
            idx + 1,
            color,
        );
    }

    // Save as PNG
    let output_path = common::test_utils::test_output_path("star_detection_visual.png");
    imag_image.save_file(&output_path).unwrap();

    println!("Saved visualization to: {:?}", output_path);
    println!("\nStar detection summary:");
    println!("  Total stars found: {}", stars.len());
    println!("  Stars drawn: {}", top_stars.len());

    // Statistics for drawn stars
    let avg_fwhm: f32 = top_stars.iter().map(|s| s.fwhm).sum::<f32>() / top_stars.len() as f32;
    let avg_snr: f32 = top_stars.iter().map(|s| s.snr).sum::<f32>() / top_stars.len() as f32;
    let avg_ecc: f32 =
        top_stars.iter().map(|s| s.eccentricity).sum::<f32>() / top_stars.len() as f32;

    println!("\nTop {} stars statistics:", top_stars.len());
    println!("  Average FWHM: {:.2} pixels", avg_fwhm);
    println!("  Average SNR: {:.1}", avg_snr);
    println!("  Average eccentricity: {:.3}", avg_ecc);
    println!(
        "  SNR range: {:.1} - {:.1}",
        top_stars.last().unwrap().snr,
        top_stars.first().unwrap().snr
    );
}
