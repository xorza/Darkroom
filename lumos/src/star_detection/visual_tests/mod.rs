//! Visual tests for star detection - generates debug images for inspection.

mod debug_steps;
pub mod generators;
pub mod output;
mod subpixel_accuracy;
mod synthetic;

// Algorithm stage tests
mod stage_tests;
// Pipeline tests
mod pipeline_tests;

use crate::AstroImage;
use crate::astro_image::{AstroImageMetadata, ImageDimensions};
use crate::star_detection::{StarDetectionConfig, find_stars};
use crate::testing::{calibration_dir, init_tracing};
use image::{GrayImage, Rgb, RgbImage};
use imageproc::drawing::{draw_cross_mut, draw_hollow_circle_mut};
use synthetic::{SyntheticFieldConfig, SyntheticStar, generate_star_field};

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

    // Find stars
    let config = StarDetectionConfig::default();
    let result = find_stars(&image, &config);
    let stars = result.stars;
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

/// Convert f32 grayscale pixels to u8 grayscale image.
fn to_gray_image(pixels: &[f32], width: usize, height: usize) -> GrayImage {
    let bytes: Vec<u8> = pixels
        .iter()
        .map(|&p| (p.clamp(0.0, 1.0) * 255.0) as u8)
        .collect();
    GrayImage::from_raw(width as u32, height as u32, bytes).unwrap()
}

#[test]
fn test_synthetic_star_detection() {
    init_tracing();

    // Create a simple synthetic star field
    let config = SyntheticFieldConfig {
        width: 256,
        height: 256,
        background: 0.1,
        noise_sigma: 0.02,
    };

    // Place stars at known positions
    let true_stars = vec![
        SyntheticStar::new(64.0, 64.0, 0.8, 3.0),   // bright star
        SyntheticStar::new(192.0, 64.0, 0.6, 2.5),  // medium star
        SyntheticStar::new(64.0, 192.0, 0.4, 2.0),  // dim star
        SyntheticStar::new(192.0, 192.0, 0.7, 3.5), // bright, wider star
        SyntheticStar::new(128.0, 128.0, 0.5, 2.0), // center star
    ];

    println!("Generating synthetic star field...");
    println!("  Image size: {}x{}", config.width, config.height);
    println!("  Background: {}", config.background);
    println!("  Noise sigma: {}", config.noise_sigma);
    println!("  Number of stars: {}", true_stars.len());

    for (i, star) in true_stars.iter().enumerate() {
        println!(
            "  Star {}: pos=({:.1}, {:.1}) brightness={:.2} sigma={:.1} fwhm={:.1}",
            i + 1,
            star.x,
            star.y,
            star.brightness,
            star.sigma,
            star.fwhm()
        );
    }

    let pixels = generate_star_field(&config, &true_stars);

    // Save the input image
    let input_image = to_gray_image(&pixels, config.width, config.height);
    let input_path = common::test_utils::test_output_path("synthetic_input.png");
    input_image.save(&input_path).unwrap();
    println!("\nSaved input image to: {:?}", input_path);

    // Run star detection with higher SNR threshold to reject noise
    let detection_config = StarDetectionConfig {
        detection_sigma: 3.0,
        min_area: 5,
        max_area: 500,
        min_snr: 20.0, // Higher threshold to reject false positives
        ..Default::default()
    };

    let image = AstroImage {
        pixels: pixels.clone(),
        dimensions: ImageDimensions::new(config.width, config.height, 1),
        metadata: AstroImageMetadata::default(),
    };
    let result = find_stars(&image, &detection_config);
    let detected_stars = result.stars;
    println!("\nDetected {} stars", detected_stars.len());

    for (i, star) in detected_stars.iter().enumerate() {
        println!(
            "  Detected {}: pos=({:.1}, {:.1}) flux={:.2} fwhm={:.1} snr={:.1}",
            i + 1,
            star.x,
            star.y,
            star.flux,
            star.fwhm,
            star.snr
        );
    }

    // Create output image with detections marked
    let mut output_image = RgbImage::from_fn(config.width as u32, config.height as u32, |x, y| {
        let idx = y as usize * config.width + x as usize;
        let v = (pixels[idx].clamp(0.0, 1.0) * 255.0) as u8;
        Rgb([v, v, v])
    });

    // Draw true star positions in blue
    let blue = Rgb([0u8, 100, 255]);
    for star in &true_stars {
        let cx = star.x.round() as i32;
        let cy = star.y.round() as i32;
        let radius = (star.fwhm() * 1.5) as i32;
        draw_hollow_circle_mut(&mut output_image, (cx, cy), radius, blue);
    }

    // Draw detected stars in green
    let green = Rgb([0u8, 255, 0]);
    for star in &detected_stars {
        let cx = star.x.round() as i32;
        let cy = star.y.round() as i32;
        draw_cross_mut(&mut output_image, green, cx, cy);
        let radius = (star.fwhm * 0.5).max(3.0) as i32;
        draw_hollow_circle_mut(&mut output_image, (cx, cy), radius, green);
    }

    let output_path = common::test_utils::test_output_path("synthetic_detection.png");
    output_image.save(&output_path).unwrap();
    println!("\nSaved detection result to: {:?}", output_path);
    println!("  Blue circles = true star positions");
    println!("  Green crosses/circles = detected stars");

    // Verify detection accuracy
    let mut matched = 0;
    for true_star in &true_stars {
        let closest = detected_stars.iter().min_by(|a, b| {
            let da = (a.x - true_star.x).powi(2) + (a.y - true_star.y).powi(2);
            let db = (b.x - true_star.x).powi(2) + (b.y - true_star.y).powi(2);
            da.partial_cmp(&db).unwrap()
        });

        if let Some(det) = closest {
            let dist = ((det.x - true_star.x).powi(2) + (det.y - true_star.y).powi(2)).sqrt();
            if dist < 3.0 {
                matched += 1;
                println!(
                    "  Matched star at ({:.1}, {:.1}): detected at ({:.1}, {:.1}), error={:.2}px",
                    true_star.x, true_star.y, det.x, det.y, dist
                );
            } else {
                println!(
                    "  MISSED star at ({:.1}, {:.1}): nearest detection at ({:.1}, {:.1}), dist={:.1}px",
                    true_star.x, true_star.y, det.x, det.y, dist
                );
            }
        }
    }

    println!(
        "\nDetection rate: {}/{} ({:.0}%)",
        matched,
        true_stars.len(),
        100.0 * matched as f32 / true_stars.len() as f32
    );

    // Assert we found all stars
    assert_eq!(
        matched,
        true_stars.len(),
        "Should detect all {} synthetic stars",
        true_stars.len()
    );
}
