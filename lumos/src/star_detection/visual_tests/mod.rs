//! Visual tests for star detection - generates debug images for inspection.

use crate::AstroImage;
use crate::star_detection::{StarDetectionConfig, find_stars};
use crate::testing::{calibration_dir, init_tracing};
use image::{Rgb, RgbImage};
use imageproc::drawing::{draw_cross_mut, draw_hollow_circle_mut};

#[test]
#[cfg_attr(not(feature = "slow-tests"), ignore)]
fn test_visualize_star_detection() {
    init_tracing();

    let Some(cal_dir) = calibration_dir() else {
        eprintln!("LUMOS_CALIBRATION_DIR not set, skipping test");
        return;
    };

    let calibrated_light = cal_dir.join("calibrated_light.tiff");
    if !calibrated_light.exists() {
        eprintln!("calibrated_light.tiff not found, skipping test");
        return;
    }

    println!("Loading calibrated light: {:?}", calibrated_light);
    let imag_image =
        imaginarium::Image::read_file(&calibrated_light).expect("Failed to load image");
    let image: AstroImage = imag_image.into();
    let (width, height) = (image.dimensions.width, image.dimensions.height);
    println!("Image size: {}x{}", width, height);
    println!("Channels: {}", image.dimensions.channels);

    // Print pixel value statistics
    let min_val = image.pixels.iter().cloned().fold(f32::INFINITY, f32::min);
    let max_val = image
        .pixels
        .iter()
        .cloned()
        .fold(f32::NEG_INFINITY, f32::max);
    println!("Pixel range: {:.6} - {:.6}", min_val, max_val);

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

    // Check for duplicates (stars within 5 pixels of each other)
    let mut duplicate_count = 0;
    for i in 0..stars.len() {
        for j in (i + 1)..stars.len() {
            let dx = stars[i].x - stars[j].x;
            let dy = stars[i].y - stars[j].y;
            let dist = (dx * dx + dy * dy).sqrt();
            if dist < 10.0 {
                duplicate_count += 1;
                if duplicate_count <= 5 {
                    println!(
                        "  Close stars: ({:.1}, {:.1}) and ({:.1}, {:.1}) dist={:.1}",
                        stars[i].x, stars[i].y, stars[j].x, stars[j].y, dist
                    );
                }
            }
        }
    }
    println!("Close star pairs (within 10px): {}", duplicate_count);

    // Normalize image to 0-1 range for proper display (auto-stretch)
    let mut display_image = image.clone();
    let min_val = display_image
        .pixels
        .iter()
        .cloned()
        .fold(f32::INFINITY, f32::min);
    let max_val = display_image
        .pixels
        .iter()
        .cloned()
        .fold(f32::NEG_INFINITY, f32::max);
    let range = (max_val - min_val).max(1e-6);
    for p in &mut display_image.pixels {
        *p = (*p - min_val) / range;
    }
    println!("Normalized pixel range: {:.6} - {:.6}", 0.0, 1.0);

    // Convert AstroImage to imaginarium::Image, then to RGB_U8
    let imag_image: imaginarium::Image = display_image.into();
    let imag_image = imag_image
        .convert(imaginarium::ColorFormat::RGB_U8)
        .expect("Failed to convert to RGB_U8");

    // Convert to image crate's RgbImage for imageproc drawing
    let mut rgb_image: RgbImage =
        RgbImage::from_raw(width as u32, height as u32, imag_image.bytes().to_vec())
            .expect("Failed to create RgbImage");

    // Draw only top N best stars (by SNR)
    let max_stars = 200;
    let top_stars: Vec<_> = stars.iter().take(max_stars).collect();

    println!("Drawing top {} stars", top_stars.len());

    // Print first few stars for debugging
    for (i, star) in top_stars.iter().take(10).enumerate() {
        println!(
            "  Star {}: pos=({:.1}, {:.1}) flux={:.1} fwhm={:.1} snr={:.1} ecc={:.3}",
            i + 1,
            star.x,
            star.y,
            star.flux,
            star.fwhm,
            star.snr,
            star.eccentricity
        );
    }

    // Green color for markers
    let color = Rgb([0u8, 255, 0]);

    for star in &top_stars {
        let radius = (star.fwhm * 1.5).max(8.0) as i32;
        let cx = star.x.round() as i32;
        let cy = star.y.round() as i32;

        // Draw circle around star
        draw_hollow_circle_mut(&mut rgb_image, (cx, cy), radius, color);

        // Draw cross at centroid
        draw_cross_mut(&mut rgb_image, color, cx, cy);
    }

    // Save as PNG
    let output_path = common::test_utils::test_output_path("star_detection_visual.png");
    rgb_image.save(&output_path).expect("Failed to save image");

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
