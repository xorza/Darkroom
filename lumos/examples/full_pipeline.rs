//! Example: Full astrophotography calibration pipeline
//!
//! This example demonstrates the complete calibration workflow:
//! 1. Create master calibration frames (darks, flats, biases) from raw frames
//! 2. Calibrate light frames using the master frames
//! 3. Detect stars in calibrated images
//!
//! # Directory Structure
//!
//! Expected input structure:
//! ```text
//! $LUMOS_CALIBRATION_DIR/
//!   Darks/
//!     dark1.raf, dark2.raf, ...
//!   Flats/
//!     flat1.raf, flat2.raf, ...
//!   Bias/
//!     bias1.raf, bias2.raf, ...
//!   Lights/
//!     light1.raf, light2.raf, ...
//! ```
//!
//! Output structure:
//! ```text
//! test_output/
//!   calibration_masters/
//!     master_dark_median.tiff
//!     master_flat_median.tiff
//!     master_bias_median.tiff
//!     hot_pixel_map.bin
//!   calibrated_lights/
//!     light1_calibrated.tiff
//!     light2_calibrated.tiff
//!     ...
//!   stars_detected.tiff
//! ```
//!
//! # Usage
//!
//! ```bash
//! LUMOS_CALIBRATION_DIR=/path/to/calibration cargo run --release --example full_pipeline
//! ```

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use lumos::{
    AstroImage, CalibrationMasters, MedianConfig, ProgressCallback, StackingMethod,
    StackingProgress, StackingStage, Star, StarDetectionConfig, find_stars,
};
use tracing_subscriber::EnvFilter;

/// Maximum number of stars to draw circles around.
const MAX_STARS_TO_DRAW: usize = 100;

/// Circle color (RGB, 0.0-1.0).
const CIRCLE_COLOR: [f32; 3] = [1.0, 0.0, 0.0]; // Red

/// Circle line thickness in pixels.
const CIRCLE_THICKNESS: usize = 2;

/// Minimum radius for drawn circles (pixels).
const MIN_CIRCLE_RADIUS: f32 = 8.0;

fn main() {
    init_tracing();

    let total_start = Instant::now();

    // Get calibration directory from environment
    let calibration_dir = env::var("LUMOS_CALIBRATION_DIR")
        .map(PathBuf::from)
        .expect("LUMOS_CALIBRATION_DIR environment variable must be set");

    tracing::info!(path = %calibration_dir.display(), "Calibration directory");

    // Set up output directories
    let output_base = PathBuf::from("test_output");
    let masters_dir = output_base.join("calibration_masters");
    let calibrated_dir = output_base.join("calibrated_lights");

    fs::create_dir_all(&masters_dir).expect("Failed to create masters directory");
    fs::create_dir_all(&calibrated_dir).expect("Failed to create calibrated directory");

    // =========================================================================
    // STEP 1: Create calibration master frames
    // =========================================================================
    println!("\n{}", "=".repeat(60));
    println!("STEP 1: Creating calibration master frames");
    println!("{}\n", "=".repeat(60));

    let masters = create_calibration_masters(&calibration_dir, &masters_dir);

    // =========================================================================
    // STEP 2: Calibrate light frames
    // =========================================================================
    println!("\n{}", "=".repeat(60));
    println!("STEP 2: Calibrating light frames");
    println!("{}\n", "=".repeat(60));

    let calibrated_paths = calibrate_light_frames(&calibration_dir, &calibrated_dir, &masters);

    // =========================================================================
    // STEP 3: Detect stars in calibrated images
    // =========================================================================
    println!("\n{}", "=".repeat(60));
    println!("STEP 3: Detecting stars");
    println!("{}\n", "=".repeat(60));

    if let Some(first_calibrated) = calibrated_paths.first() {
        detect_stars_in_image(first_calibrated, &output_base);
    } else {
        tracing::warn!("No calibrated images to run star detection on");
    }

    // =========================================================================
    // Summary
    // =========================================================================
    let total_elapsed = total_start.elapsed();
    println!("\n{}", "=".repeat(60));
    println!("PIPELINE COMPLETE");
    println!("{}", "=".repeat(60));
    println!("Total time: {:.2}s", total_elapsed.as_secs_f32());
}

/// Step 1: Create calibration master frames from raw calibration frames.
fn create_calibration_masters(calibration_dir: &Path, output_dir: &Path) -> CalibrationMasters {
    let start = Instant::now();

    let config = MedianConfig::default();
    let method = StackingMethod::Median(config);
    let progress = create_progress_callback();

    tracing::info!("Creating calibration masters from raw frames...");

    let masters = CalibrationMasters::from_directory(calibration_dir, method, progress)
        .expect("Failed to create calibration masters");

    let elapsed = start.elapsed();

    // Report what was created
    if let Some(ref dark) = masters.master_dark {
        tracing::info!(
            width = dark.dimensions.width,
            height = dark.dimensions.height,
            "Master dark created"
        );
    } else {
        tracing::warn!("No master dark created (no Darks subdirectory)");
    }

    if let Some(ref flat) = masters.master_flat {
        tracing::info!(
            width = flat.dimensions.width,
            height = flat.dimensions.height,
            "Master flat created"
        );
    } else {
        tracing::warn!("No master flat created (no Flats subdirectory)");
    }

    if let Some(ref bias) = masters.master_bias {
        tracing::info!(
            width = bias.dimensions.width,
            height = bias.dimensions.height,
            "Master bias created"
        );
    } else {
        tracing::warn!("No master bias created (no Bias subdirectory)");
    }

    if let Some(ref hot_pixels) = masters.hot_pixel_map {
        tracing::info!(
            count = hot_pixels.count,
            percentage = format!("{:.4}%", hot_pixels.percentage()),
            "Hot pixels detected"
        );
    }

    // Save masters
    masters
        .save_to_directory(output_dir)
        .expect("Failed to save calibration masters");

    tracing::info!(
        elapsed_secs = format!("{:.2}", elapsed.as_secs_f32()),
        "Step 1 complete: Calibration masters created"
    );

    masters
}

/// Step 2: Calibrate light frames using master calibration frames.
fn calibrate_light_frames(
    calibration_dir: &Path,
    output_dir: &Path,
    masters: &CalibrationMasters,
) -> Vec<PathBuf> {
    let lights_dir = calibration_dir.join("Lights");

    if !lights_dir.exists() {
        tracing::warn!("Lights directory does not exist: {}", lights_dir.display());
        return Vec::new();
    }

    // Find light frames
    let light_paths = common::file_utils::astro_image_files(&lights_dir);
    if light_paths.is_empty() {
        tracing::warn!("No light frames found in {}", lights_dir.display());
        return Vec::new();
    }

    tracing::info!(count = light_paths.len(), "Found light frames to calibrate");

    let start = Instant::now();
    let mut calibrated_paths = Vec::new();

    for path in &light_paths {
        let filename = path.file_stem().unwrap().to_string_lossy();
        tracing::info!(file = %filename, "Calibrating");

        // Load the light frame
        let mut light = match AstroImage::from_file(path) {
            Ok(img) => img,
            Err(e) => {
                tracing::error!(file = %filename, error = %e, "Failed to load");
                continue;
            }
        };

        // Calibrate
        masters.calibrate(&mut light);

        // Save calibrated frame
        let output_path = output_dir.join(format!("{}_calibrated.tiff", filename));
        let img: imaginarium::Image = light.into();
        if let Err(e) = img.save_file(&output_path) {
            tracing::error!(file = %filename, error = %e, "Failed to save");
            continue;
        }

        tracing::debug!(file = %filename, "Saved");
        calibrated_paths.push(output_path);
    }

    let elapsed = start.elapsed();
    tracing::info!(
        successful = calibrated_paths.len(),
        failed = light_paths.len() - calibrated_paths.len(),
        elapsed_secs = format!("{:.2}", elapsed.as_secs_f32()),
        "Step 2 complete: Light frames calibrated"
    );

    calibrated_paths
}

/// Step 3: Detect stars in a calibrated image.
fn detect_stars_in_image(image_path: &Path, output_dir: &Path) {
    tracing::info!(path = %image_path.display(), "Loading image for star detection");

    // Load the image
    let image = match AstroImage::from_file(image_path) {
        Ok(img) => img,
        Err(e) => {
            tracing::error!(error = %e, "Failed to load image");
            return;
        }
    };

    tracing::info!(
        width = image.dimensions.width,
        height = image.dimensions.height,
        channels = image.dimensions.channels,
        "Image loaded"
    );

    // Configure star detection
    let config = StarDetectionConfig {
        edge_margin: 20,
        min_snr: 15.0,
        max_eccentricity: 0.5,
        ..StarDetectionConfig::default()
    };

    // Run star detection
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

    if result.stars.is_empty() {
        tracing::warn!("No stars detected!");
        return;
    }

    // Filter out stars in the bottom portion of the image (horizon, ground)
    let height_cutoff = image.dimensions.height as f32 * 0.7;
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

    // Take the best stars (sorted by flux, brightest first)
    let best_stars: Vec<&Star> = sky_stars.iter().copied().take(MAX_STARS_TO_DRAW).collect();

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
    let output_path = output_dir.join("stars_detected.tiff");
    let img: imaginarium::Image = output_image.into();
    img.save_file(&output_path)
        .expect("Failed to save output image");

    tracing::info!(
        path = %output_path.display(),
        stars_marked = best_stars.len(),
        "Step 3 complete: Stars detected and marked"
    );
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

            if dist_sq >= min_r_sq && dist_sq <= max_r_sq {
                let idx = (y * width + x) * channels;
                if channels == 1 {
                    image.pixels[idx] = 0.299 * color[0] + 0.587 * color[1] + 0.114 * color[2];
                } else {
                    image.pixels[idx] = color[0];
                    image.pixels[idx + 1] = color[1];
                    image.pixels[idx + 2] = color[2];
                }
            }
        }
    }
}

/// Create a progress callback that logs to console.
fn create_progress_callback() -> ProgressCallback {
    let callback: Arc<dyn Fn(StackingProgress) + Send + Sync> =
        Arc::new(move |progress: StackingProgress| {
            let stage = match progress.stage {
                StackingStage::Loading => "Loading",
                StackingStage::Processing => "Processing",
            };
            let percent = if progress.total > 0 {
                (progress.current as f32 / progress.total as f32) * 100.0
            } else {
                0.0
            };
            tracing::debug!(
                stage,
                current = progress.current,
                total = progress.total,
                percent = format!("{:.1}%", percent),
                "Progress"
            );
        });
    ProgressCallback::from(callback)
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
