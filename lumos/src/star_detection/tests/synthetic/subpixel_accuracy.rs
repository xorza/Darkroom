//! Sub-pixel accuracy tests for star detection.
//!
//! Tests the algorithm's ability to detect sub-pixel shifts between images,
//! which is critical for image registration and stacking.

use super::{SyntheticFieldConfig, SyntheticStar, generate_star_field};
use crate::AstroImage;

use crate::star_detection::tests::common::save_image_png;
use crate::star_detection::{Star, StarDetectionConfig, find_stars};
use imaginarium::Color;
use imaginarium::drawing::{draw_circle, draw_cross};

fn make_grayscale_image(pixels: Vec<f32>, width: usize, height: usize) -> AstroImage {
    AstroImage::from_pixels(width, height, 1, pixels)
}

/// Match detected stars between two images based on proximity.
/// Returns pairs of (star1, star2) that are within `max_distance` pixels.
fn match_stars(stars1: &[Star], stars2: &[Star], max_distance: f32) -> Vec<(Star, Star)> {
    let max_dist_sq = max_distance * max_distance;
    let mut matched = Vec::new();
    let mut used2 = vec![false; stars2.len()];

    for s1 in stars1 {
        let mut best_idx = None;
        let mut best_dist_sq = max_dist_sq;

        for (j, s2) in stars2.iter().enumerate() {
            if used2[j] {
                continue;
            }
            let dx = s1.x - s2.x;
            let dy = s1.y - s2.y;
            let dist_sq = dx * dx + dy * dy;
            if dist_sq < best_dist_sq {
                best_dist_sq = dist_sq;
                best_idx = Some(j);
            }
        }

        if let Some(j) = best_idx {
            matched.push((*s1, stars2[j]));
            used2[j] = true;
        }
    }

    matched
}

/// Compute the median shift between matched star pairs.
fn compute_median_shift(pairs: &[(Star, Star)]) -> (f32, f32) {
    if pairs.is_empty() {
        return (0.0, 0.0);
    }

    let mut dx_values: Vec<f32> = pairs.iter().map(|(s1, s2)| s2.x - s1.x).collect();
    let mut dy_values: Vec<f32> = pairs.iter().map(|(s1, s2)| s2.y - s1.y).collect();

    dx_values.sort_by(|a, b| a.partial_cmp(b).unwrap());
    dy_values.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let median_dx = dx_values[dx_values.len() / 2];
    let median_dy = dy_values[dy_values.len() / 2];

    (median_dx, median_dy)
}

/// Test that the star detection algorithm can accurately recover sub-pixel shifts.
#[test]
fn test_subpixel_shift_detection() {
    // Known sub-pixel shift to apply
    let shift_x = 0.15;
    let shift_y = 0.23;

    let config = SyntheticFieldConfig {
        width: 256,
        height: 256,
        background: 0.1,
        noise_sigma: 0.015,
    };

    // Create stars at various positions with different brightnesses
    // Scaled for 256x256 image with margin for edge detection
    let base_stars = vec![
        SyntheticStar::new(40.0, 40.0, 0.9, 3.0),
        SyntheticStar::new(100.0, 40.0, 0.8, 2.8),
        SyntheticStar::new(170.0, 50.0, 0.7, 3.2),
        SyntheticStar::new(220.0, 75.0, 0.75, 2.5),
        SyntheticStar::new(50.0, 100.0, 0.65, 2.6),
        SyntheticStar::new(128.0, 128.0, 0.85, 3.0),
        SyntheticStar::new(200.0, 140.0, 0.6, 2.4),
        SyntheticStar::new(40.0, 175.0, 0.7, 2.8),
        SyntheticStar::new(100.0, 200.0, 0.55, 2.2),
        SyntheticStar::new(175.0, 190.0, 0.8, 3.1),
        SyntheticStar::new(220.0, 210.0, 0.72, 2.7),
        SyntheticStar::new(75.0, 220.0, 0.68, 2.9),
    ];

    // Create shifted stars
    let shifted_stars: Vec<SyntheticStar> = base_stars
        .iter()
        .map(|s| SyntheticStar::new(s.x + shift_x, s.y + shift_y, s.brightness, s.sigma))
        .collect();

    println!("Generating two synthetic star fields...");
    println!("  Image size: {}x{}", config.width, config.height);
    println!("  Number of stars: {}", base_stars.len());
    println!("  Applied shift: dx={:.3}, dy={:.3}", shift_x, shift_y);

    let pixels1 = generate_star_field(&config, &base_stars);
    let pixels2 = generate_star_field(&config, &shifted_stars);

    // Run star detection on both images
    let detection_config = StarDetectionConfig::default();

    let image1 = make_grayscale_image(pixels1.clone(), config.width, config.height);
    let image2 = make_grayscale_image(pixels2.clone(), config.width, config.height);
    let detected1 = find_stars(&image1, &detection_config).stars;
    let detected2 = find_stars(&image2, &detection_config).stars;

    println!("\nDetection results:");
    println!("  Image 1: {} stars detected", detected1.len());
    println!("  Image 2: {} stars detected", detected2.len());

    // Match stars between images
    let pairs = match_stars(&detected1, &detected2, 5.0);
    println!("  Matched pairs: {}", pairs.len());

    assert!(
        pairs.len() >= base_stars.len() - 2,
        "Should match most stars: got {} pairs, expected at least {}",
        pairs.len(),
        base_stars.len() - 2
    );

    // Compute detected shift
    let (detected_dx, detected_dy) = compute_median_shift(&pairs);
    println!("\nShift measurement:");
    println!("  True shift:     dx={:.4}, dy={:.4}", shift_x, shift_y);
    println!(
        "  Detected shift: dx={:.4}, dy={:.4}",
        detected_dx, detected_dy
    );

    let error_x = (detected_dx - shift_x).abs();
    let error_y = (detected_dy - shift_y).abs();
    let total_error = (error_x * error_x + error_y * error_y).sqrt();

    println!(
        "  Error: dx={:.4}, dy={:.4}, total={:.4}",
        error_x, error_y, total_error
    );

    // Print per-star errors
    println!("\nPer-star measurements:");
    for (i, (s1, s2)) in pairs.iter().enumerate() {
        let dx = s2.x - s1.x;
        let dy = s2.y - s1.y;
        let err_x = (dx - shift_x).abs();
        let err_y = (dy - shift_y).abs();
        println!(
            "  Star {}: pos1=({:.2}, {:.2}) pos2=({:.2}, {:.2}) shift=({:.3}, {:.3}) err=({:.3}, {:.3})",
            i + 1,
            s1.x,
            s1.y,
            s2.x,
            s2.y,
            dx,
            dy,
            err_x,
            err_y
        );
    }

    // Assert sub-pixel accuracy (should be within 0.1 pixel)
    let tolerance = 0.05;
    assert!(
        error_x < tolerance,
        "X shift error {:.4} exceeds tolerance {:.4}",
        error_x,
        tolerance
    );
    assert!(
        error_y < tolerance,
        "Y shift error {:.4} exceeds tolerance {:.4}",
        error_y,
        tolerance
    );

    println!(
        "\nSUCCESS: Sub-pixel shift detected within {:.2} pixel tolerance",
        tolerance
    );

    // Save visualization
    save_subpixel_visualization(
        &pixels1,
        &pixels2,
        &detected1,
        &detected2,
        &pairs,
        config.width,
        config.height,
        shift_x,
        shift_y,
        detected_dx,
        detected_dy,
    );
}

/// Test accuracy across a range of sub-pixel offsets.
#[test]
fn test_subpixel_accuracy_sweep() {
    let config = SyntheticFieldConfig {
        width: 256,
        height: 256,
        background: 0.1,
        noise_sigma: 0.01,
    };

    // Create a grid of stars
    let base_stars: Vec<SyntheticStar> = (0..4)
        .flat_map(|row| {
            (0..4).map(move |col| {
                let x = 40.0 + col as f32 * 50.0;
                let y = 40.0 + row as f32 * 50.0;
                let brightness = 0.6 + (row + col) as f32 * 0.05;
                SyntheticStar::new(x, y, brightness, 2.5)
            })
        })
        .collect();

    let detection_config = StarDetectionConfig::default();

    // Test various sub-pixel shifts
    let test_shifts = [
        (0.1, 0.0),
        (0.0, 0.1),
        (0.1, 0.1),
        (0.25, 0.25),
        (0.33, 0.17),
        (0.5, 0.5),
        (-0.2, 0.3),
    ];

    println!(
        "Testing sub-pixel accuracy across {} shift values...\n",
        test_shifts.len()
    );
    println!(
        "{:>10} {:>10} {:>10} {:>10} {:>10}",
        "True dX", "True dY", "Det dX", "Det dY", "Error"
    );
    println!("{:-<55}", "");

    let mut max_error = 0.0f32;
    let mut all_passed = true;

    let pixels1 = generate_star_field(&config, &base_stars);
    let image1 = make_grayscale_image(pixels1, config.width, config.height);
    let detected1 = find_stars(&image1, &detection_config).stars;

    for (shift_x, shift_y) in test_shifts {
        let shifted_stars: Vec<SyntheticStar> = base_stars
            .iter()
            .map(|s| SyntheticStar::new(s.x + shift_x, s.y + shift_y, s.brightness, s.sigma))
            .collect();

        let pixels2 = generate_star_field(&config, &shifted_stars);
        let image2 = make_grayscale_image(pixels2, config.width, config.height);
        let detected2 = find_stars(&image2, &detection_config).stars;

        let pairs = match_stars(&detected1, &detected2, 3.0);
        let (detected_dx, detected_dy) = compute_median_shift(&pairs);

        let error = ((detected_dx - shift_x).powi(2) + (detected_dy - shift_y).powi(2)).sqrt();
        max_error = max_error.max(error);

        let status = if error < 0.1 { "OK" } else { "FAIL" };
        println!(
            "{:>10.3} {:>10.3} {:>10.3} {:>10.3} {:>10.4} {}",
            shift_x, shift_y, detected_dx, detected_dy, error, status
        );

        if error >= 0.1 {
            all_passed = false;
        }
    }

    println!("{:-<55}", "");
    println!("Maximum error: {:.4} pixels", max_error);

    assert!(
        all_passed,
        "Some shift measurements exceeded 0.1 pixel tolerance"
    );
    println!("\nSUCCESS: All sub-pixel shifts detected within 0.1 pixel tolerance");
}

#[allow(clippy::too_many_arguments)]
fn save_subpixel_visualization(
    pixels1: &[f32],
    pixels2: &[f32],
    detected1: &[Star],
    detected2: &[Star],
    pairs: &[(Star, Star)],
    width: usize,
    height: usize,
    true_dx: f32,
    true_dy: f32,
    detected_dx: f32,
    detected_dy: f32,
) {
    // Create side-by-side comparison using imaginarium
    let combined_width = width * 2 + 20; // 20px gap

    // Create combined image
    let desc = imaginarium::ImageDesc::new_packed(
        combined_width,
        height,
        imaginarium::ColorFormat::RGB_F32,
    );
    let mut output = imaginarium::Image::new_black(desc).unwrap();
    let pixels_out: &mut [f32] = bytemuck::cast_slice_mut(output.bytes_mut());

    // Fill gap with dark gray
    for y in 0..height {
        for x in width..(width + 20) {
            let idx = (y * combined_width + x) * 3;
            pixels_out[idx] = 0.15;
            pixels_out[idx + 1] = 0.15;
            pixels_out[idx + 2] = 0.15;
        }
    }

    // Copy image 1 to left side
    for y in 0..height {
        for x in 0..width {
            let v = pixels1[y * width + x].clamp(0.0, 1.0);
            let idx = (y * combined_width + x) * 3;
            pixels_out[idx] = v;
            pixels_out[idx + 1] = v;
            pixels_out[idx + 2] = v;
        }
    }

    // Copy image 2 to right side
    for y in 0..height {
        for x in 0..width {
            let v = pixels2[y * width + x].clamp(0.0, 1.0);
            let idx = (y * combined_width + (x + width + 20)) * 3;
            pixels_out[idx] = v;
            pixels_out[idx + 1] = v;
            pixels_out[idx + 2] = v;
        }
    }

    // Draw detected stars
    let green = Color::GREEN;
    let cyan = Color::CYAN;
    let yellow = Color::YELLOW;

    for star in detected1 {
        let cx = star.x;
        let cy = star.y;
        draw_cross(&mut output, cx, cy, 3.0, green, 1.0);
    }

    for star in detected2 {
        let cx = star.x + width as f32 + 20.0;
        let cy = star.y;
        draw_cross(&mut output, cx, cy, 3.0, green, 1.0);
    }

    // Draw circles around matched pairs
    for (s1, s2) in pairs {
        let cx1 = s1.x;
        let cy1 = s1.y;
        draw_circle(&mut output, cx1, cy1, 8.0, cyan, 1.0);

        let cx2 = s2.x + width as f32 + 20.0;
        let cy2 = s2.y;
        draw_circle(&mut output, cx2, cy2, 8.0, yellow, 1.0);
    }

    let output_path =
        common::test_utils::test_output_path("synthetic_starfield/subpixel_shift_test.png");
    save_image_png(output, &output_path);
    println!("\nSaved visualization to: {:?}", output_path);
    println!("  Left: Image 1 (reference)");
    println!(
        "  Right: Image 2 (shifted by dx={:.3}, dy={:.3})",
        true_dx, true_dy
    );
    println!(
        "  Detected shift: dx={:.4}, dy={:.4}",
        detected_dx, detected_dy
    );
}
