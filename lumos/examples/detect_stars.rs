//! Example: Detect stars in a calibrated light frame
//!
//! This example reads the first calibrated light frame from LUMOS_CALIBRATION_DIR/calibrated_lights/,
//! runs star detection, and draws circles around the best detected stars.
//!
//! # Directory Structure
//!
//! Expected input:
//! ```text
//! $LUMOS_CALIBRATION_DIR/
//!   calibrated_lights/
//!     light1_calibrated.tiff, ...
//! ```
//!
//! Output:
//! ```text
//! test_output/
//!   stars_detected.png
//! ```
//!
//! # Usage
//!
//! ```bash
//! LUMOS_CALIBRATION_DIR=/path/to/calibration cargo run --example detect_stars
//! ```

use std::env;
use std::path::PathBuf;
use std::time::Instant;

use lumos::{AstroImage, Star, StarDetectionConfig, find_stars};
use tracing_subscriber::EnvFilter;

/// Maximum number of stars to draw circles around.
const MAX_STARS_TO_DRAW: usize = 100;

/// Circle color (RGB, 0.0-1.0).
const CIRCLE_COLOR: [f32; 3] = [1.0, 0.0, 0.0]; // Red - more visible

/// Circle line thickness in pixels.
const CIRCLE_THICKNESS: usize = 2;

/// Minimum radius for drawn circles (pixels).
const MIN_CIRCLE_RADIUS: f32 = 8.0;

fn main() {
    // Initialize tracing for console output
    init_tracing();

    // Get calibration directory from environment
    let calibration_dir = env::var("LUMOS_CALIBRATION_DIR")
        .map(PathBuf::from)
        .expect("LUMOS_CALIBRATION_DIR environment variable must be set");

    let calibrated_lights_dir = calibration_dir.join("calibrated_lights");

    // Verify directory exists
    assert!(
        calibrated_lights_dir.exists(),
        "Calibrated lights directory does not exist: {}. Run calibrate_lights example first.",
        calibrated_lights_dir.display()
    );

    let light_path =
        calibration_dir.join("calibrated_lights/03_DSCF6807_calibrated_stretched.tiff");
    tracing::info!(path = %light_path.display(), "Loading calibrated light frame");

    // Load the image
    let image = AstroImage::from_file(light_path).expect("Failed to load image");
    tracing::info!(
        width = image.dimensions.width,
        height = image.dimensions.height,
        channels = image.dimensions.channels,
        "Image loaded"
    );

    // Configure star detection - filter stars in the bottom portion of the image
    // (often contains horizon, water reflections, or ground)
    let config = StarDetectionConfig {
        edge_margin: 20,       // Larger edge margin
        min_snr: 15.0,         // Higher SNR threshold
        max_eccentricity: 0.5, // Stricter roundness
        ..StarDetectionConfig::default()
    };

    // Run star detection
    tracing::info!("Running star detection...");
    let start = Instant::now();
    let result = find_stars(&image, &config);
    let elapsed = start.elapsed();

    tracing::info!(
        stars_detected = result.stars.len(),
        elapsed_ms = elapsed.as_millis(),
        median_fwhm = format!("{:.2}", result.diagnostics.median_fwhm),
        median_snr = format!("{:.1}", result.diagnostics.median_snr),
        "Star detection complete"
    );

    // Report detection diagnostics
    tracing::debug!(
        candidates = result.diagnostics.candidates_after_filtering,
        rejected_low_snr = result.diagnostics.rejected_low_snr,
        rejected_saturated = result.diagnostics.rejected_saturated,
        rejected_cosmic_rays = result.diagnostics.rejected_cosmic_rays,
        rejected_eccentricity = result.diagnostics.rejected_high_eccentricity,
        "Detection diagnostics"
    );

    if result.stars.is_empty() {
        tracing::warn!("No stars detected!");
        return;
    }

    // Filter out stars in the bottom third of the image (horizon, water, ground)
    let height_cutoff = (image.dimensions.height as f32 * 0.7) as f32;
    let sky_stars: Vec<&Star> = result
        .stars
        .iter()
        .filter(|s| s.y < height_cutoff)
        .collect();

    tracing::info!(
        total = result.stars.len(),
        sky_only = sky_stars.len(),
        "Filtered stars above horizon"
    );

    // Take the best stars (already sorted by flux, brightest first)
    let best_stars: Vec<&Star> = sky_stars.iter().copied().take(MAX_STARS_TO_DRAW).collect();
    tracing::info!(
        count = best_stars.len(),
        "Drawing circles around best stars"
    );

    // Print info about the brightest stars
    println!("\nTop 10 brightest stars:");
    println!(
        "{:>4}  {:>8}  {:>8}  {:>8}  {:>6}  {:>6}",
        "Rank", "X", "Y", "Flux", "FWHM", "SNR"
    );
    for (i, star) in best_stars.iter().take(10).enumerate() {
        println!(
            "{:>4}  {:>8.2}  {:>8.2}  {:>8.1}  {:>6.2}  {:>6.1}",
            i + 1,
            star.x,
            star.y,
            star.flux,
            star.fwhm,
            star.snr
        );
    }

    // Draw circles on the image
    let mut output_image = image.clone();
    for star in &best_stars {
        let radius = (star.fwhm * 2.0).max(MIN_CIRCLE_RADIUS);
        draw_circle(
            &mut output_image,
            star.x,
            star.y,
            radius,
            &CIRCLE_COLOR,
            CIRCLE_THICKNESS,
        );
    }

    // Save output image
    let output_dir = PathBuf::from("test_output");
    std::fs::create_dir_all(&output_dir).expect("Failed to create output directory");
    let output_path = output_dir.join("stars_detected.tiff");

    // Convert to imaginarium Image and save
    let img: imaginarium::Image = output_image.into();
    img.save_file(&output_path)
        .expect("Failed to save output image");

    tracing::info!(path = %output_path.display(), "Output saved");
    println!("\nOutput: {}", output_path.display());
}

/// Draw a circle on an AstroImage at the given center with the given radius.
fn draw_circle(
    image: &mut AstroImage,
    cx: f32,
    cy: f32,
    radius: f32,
    color: &[f32; 3],
    thickness: usize,
) {
    let width = image.dimensions.width;
    let height = image.dimensions.height;
    let channels = image.dimensions.channels;

    let min_radius = (radius - thickness as f32 / 2.0).max(1.0);
    let max_radius = radius + thickness as f32 / 2.0;
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

            // Check if pixel is within the ring (between min and max radius)
            if dist_sq >= min_r_sq && dist_sq <= max_r_sq {
                let idx = (y * width + x) * channels;
                if channels == 1 {
                    // Grayscale: use luminance of color
                    image.pixels[idx] = 0.299 * color[0] + 0.587 * color[1] + 0.114 * color[2];
                } else {
                    // RGB
                    image.pixels[idx] = color[0];
                    image.pixels[idx + 1] = color[1];
                    image.pixels[idx + 2] = color[2];
                }
            }
        }
    }
}

/// Initialize tracing subscriber with console output.
fn init_tracing() {
    use tracing_subscriber::prelude::*;

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_target(true)
        .with_level(true)
        .with_thread_ids(false)
        .with_file(false)
        .with_line_number(false);

    tracing_subscriber::registry()
        .with(filter)
        .with(fmt_layer)
        .init();
}
