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
