//! Example: Full astrophotography calibration pipeline
//!
//! This example demonstrates the complete calibration workflow:
//! 1. Create master calibration frames (darks, flats, biases) from raw frames
//! 2. Calibrate light frames using the master frames
//! 3. Detect stars in calibrated images
//! 4. Register (align) all lights to a reference frame
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
//!   registered_lights/
//!     light1_registered.tiff (reference frame, copied as-is)
//!     light2_registered.tiff
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
    AstroImage, CalibrationMasters, InterpolationMethod, MedianConfig, ProgressCallback,
    RegistrationConfig, Registrator, StackingMethod, StackingProgress, StackingStage, Star,
    StarDetectionConfig, find_stars,
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

/// Maximum stars to use for registration.
const MAX_STARS_FOR_REGISTRATION: usize = 200;

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
    let registered_dir = output_base.join("registered_lights");

    fs::create_dir_all(&masters_dir).expect("Failed to create masters directory");
    fs::create_dir_all(&calibrated_dir).expect("Failed to create calibrated directory");
    fs::create_dir_all(&registered_dir).expect("Failed to create registered directory");

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
    // STEP 3: Detect stars in all calibrated images
    // =========================================================================
    println!("\n{}", "=".repeat(60));
    println!("STEP 3: Detecting stars in all calibrated images");
    println!("{}\n", "=".repeat(60));

    let stars_per_image = detect_stars_in_all_images(&calibrated_paths);

    // =========================================================================
    // STEP 4: Register all lights to reference frame
    // =========================================================================
    println!("\n{}", "=".repeat(60));
    println!("STEP 4: Registering all lights to reference frame");
    println!("{}\n", "=".repeat(60));

    register_all_lights(
        &calibrated_paths,
        &stars_per_image,
        &registered_dir,
        &output_base,
    );

    // =========================================================================
    // Summary
    // =========================================================================
    let total_elapsed = total_start.elapsed();
    println!("\n{}", "=".repeat(60));
    println!("PIPELINE COMPLETE");
    println!("{}", "=".repeat(60));
    println!("Total time: {:.2}s", total_elapsed.as_secs_f32());
}

/// Step 1: Load or create calibration master frames.
fn create_calibration_masters(calibration_dir: &Path, output_dir: &Path) -> CalibrationMasters {
    let start = Instant::now();

    let config = MedianConfig::default();
    let method = StackingMethod::Median(config);

    // First, try to load existing masters from the output directory
    if let Ok(masters) = CalibrationMasters::load_from_directory(output_dir, method.clone())
        && (masters.master_dark.is_some()
            || masters.master_flat.is_some()
            || masters.master_bias.is_some())
    {
        tracing::info!(
            "Loaded existing calibration masters from {}",
            output_dir.display()
        );
        return masters;
    }

    // No existing masters found, create from raw frames
    let progress = create_progress_callback();
    tracing::info!("Creating calibration masters from raw frames...");

    let masters = CalibrationMasters::from_directory(calibration_dir, method, progress)
        .expect("Failed to create calibration masters");

    let elapsed = start.elapsed();

    // Report what was created
    if let Some(ref dark) = masters.master_dark {
        tracing::info!(
            width = dark.dimensions().width,
            height = dark.dimensions().height,
            "Master dark created"
        );
    } else {
        tracing::warn!("No master dark created (no Darks subdirectory)");
    }

    if let Some(ref flat) = masters.master_flat {
        tracing::info!(
            width = flat.dimensions().width,
            height = flat.dimensions().height,
            "Master flat created"
        );
    } else {
        tracing::warn!("No master flat created (no Flats subdirectory)");
    }

    if let Some(ref bias) = masters.master_bias {
        tracing::info!(
            width = bias.dimensions().width,
            height = bias.dimensions().height,
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
/// Returns paths to calibrated TIFF files.
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

    let mut skipped_count = 0usize;
    let mut calibrated_count = 0usize;

    for path in &light_paths {
        let filename = path.file_stem().unwrap().to_string_lossy().to_string();
        let output_path = output_dir.join(format!("{}_calibrated.tiff", filename));

        // Check if calibrated version already exists
        if output_path.exists() {
            tracing::debug!(file = %filename, "Calibrated frame already exists, skipping");
            calibrated_paths.push(output_path);
            skipped_count += 1;
            continue;
        }

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
        let img: imaginarium::Image = light.into();
        if let Err(e) = img.save_file(&output_path) {
            tracing::error!(file = %filename, error = %e, "Failed to save");
            continue;
        }

        tracing::debug!(file = %filename, "Saved");
        calibrated_paths.push(output_path);
        calibrated_count += 1;
    }

    let elapsed = start.elapsed();
    tracing::info!(
        skipped = skipped_count,
        calibrated = calibrated_count,
        failed = light_paths.len() - calibrated_paths.len(),
        elapsed_secs = format!("{:.2}", elapsed.as_secs_f32()),
        "Step 2 complete: Light frames processed"
    );

    calibrated_paths
}

/// Step 3: Detect stars in all calibrated images.
fn detect_stars_in_all_images(calibrated_paths: &[PathBuf]) -> Vec<Vec<Star>> {
    let start = Instant::now();

    let config = StarDetectionConfig {
        edge_margin: 20,
        min_snr: 10.0,
        max_eccentricity: 0.6,
        ..StarDetectionConfig::default()
    };

    let mut all_stars = Vec::with_capacity(calibrated_paths.len());

    for path in calibrated_paths {
        let filename = path.file_stem().unwrap().to_string_lossy();

        let image = match AstroImage::from_file(path) {
            Ok(img) => img,
            Err(e) => {
                tracing::error!(file = %filename, error = %e, "Failed to load for star detection");
                all_stars.push(Vec::new());
                continue;
            }
        };

        let result = find_stars(&image, &config);

        tracing::info!(
            file = %filename,
            stars = result.stars.len(),
            median_fwhm = format!("{:.2}", result.diagnostics.median_fwhm),
            median_snr = format!("{:.1}", result.diagnostics.median_snr),
            "Stars detected"
        );

        all_stars.push(result.stars);
    }

    let elapsed = start.elapsed();
    let total_stars: usize = all_stars.iter().map(|s| s.len()).sum();
    tracing::info!(
        images = calibrated_paths.len(),
        total_stars,
        elapsed_secs = format!("{:.2}", elapsed.as_secs_f32()),
        "Step 3 complete: Stars detected in all images"
    );

    all_stars
}

/// Step 4: Register all lights to a reference frame.
fn register_all_lights(
    calibrated_paths: &[PathBuf],
    stars_per_image: &[Vec<Star>],
    output_dir: &Path,
    output_base: &Path,
) {
    if calibrated_paths.is_empty() {
        tracing::warn!("No calibrated images to register");
        return;
    }

    let start = Instant::now();

    // Use the first image as the reference
    let ref_path = &calibrated_paths[0];
    let ref_filename = ref_path.file_stem().unwrap().to_string_lossy();
    let ref_stars = &stars_per_image[0];

    tracing::info!(
        reference = %ref_filename,
        ref_stars = ref_stars.len(),
        "Using first image as reference"
    );

    // Load reference image to get dimensions
    let ref_image = AstroImage::from_file(ref_path).expect("Failed to load reference image");
    let width = ref_image.dimensions().width;
    let height = ref_image.dimensions().height;

    // Convert reference stars to (x, y) tuples, sorted by flux (brightest first)
    let mut ref_star_positions: Vec<(f64, f64)> =
        ref_stars.iter().map(|s| (s.x as f64, s.y as f64)).collect();
    // Stars are already sorted by flux from find_stars, take the brightest
    ref_star_positions.truncate(MAX_STARS_FOR_REGISTRATION);

    // Configure registration
    let reg_config = RegistrationConfig::builder()
        .with_scale()
        .ransac_iterations(1000)
        .ransac_threshold(2.0)
        .max_stars(MAX_STARS_FOR_REGISTRATION)
        .min_matched_stars(6)
        .max_residual(3.0)
        .build();

    let registrator = Registrator::new(reg_config);

    let mut successful_registrations = 0;
    let mut failed_registrations = 0;

    for (i, (path, stars)) in calibrated_paths
        .iter()
        .zip(stars_per_image.iter())
        .enumerate()
    {
        let filename = path.file_stem().unwrap().to_string_lossy();
        let output_path = output_dir.join(format!(
            "{}_registered.tiff",
            filename.trim_end_matches("_calibrated")
        ));

        if i == 0 {
            // Reference frame - just copy it
            let img: imaginarium::Image = ref_image.clone().into();
            img.save_file(&output_path)
                .expect("Failed to save reference frame");
            tracing::info!(file = %filename, "Reference frame saved (no transformation needed)");
            successful_registrations += 1;
            continue;
        }

        // Load the target image
        let target_image = match AstroImage::from_file(path) {
            Ok(img) => img,
            Err(e) => {
                tracing::error!(file = %filename, error = %e, "Failed to load for registration");
                failed_registrations += 1;
                continue;
            }
        };

        // Convert target stars to (x, y) tuples
        let mut target_star_positions: Vec<(f64, f64)> =
            stars.iter().map(|s| (s.x as f64, s.y as f64)).collect();
        target_star_positions.truncate(MAX_STARS_FOR_REGISTRATION);

        // Register target to reference
        match registrator.register_stars(&ref_star_positions, &target_star_positions) {
            Ok(result) => {
                tracing::info!(
                    file = %filename,
                    matched_stars = result.num_inliers,
                    rms_error = format!("{:.3}", result.rms_error),
                    elapsed_ms = format!("{:.1}", result.elapsed_ms),
                    "Registration successful"
                );

                tracing::info!("Transform: {}", result.transform);

                // Warp the image to align with reference
                let warped =
                    warp_image_to_reference(&target_image, width, height, &result.transform);

                tracing::info!(
                    file = %filename,
                    "Warped image saved"
                );

                // Save registered image
                let img: imaginarium::Image = warped.into();
                img.save_file(&output_path)
                    .expect("Failed to save registered frame");

                successful_registrations += 1;
            }
            Err(e) => {
                tracing::error!(
                    file = %filename,
                    error = %e,
                    "Registration failed"
                );
                failed_registrations += 1;
            }
        }
    }

    let elapsed = start.elapsed();
    tracing::info!(
        successful = successful_registrations,
        failed = failed_registrations,
        elapsed_secs = format!("{:.2}", elapsed.as_secs_f32()),
        "Step 4 complete: All lights registered"
    );

    // Save visualization of stars detected in reference frame
    if !ref_stars.is_empty() {
        save_star_visualization(&ref_image, ref_stars, output_base);
    }
}

/// Warp an image to align with the reference frame.
fn warp_image_to_reference(
    image: &AstroImage,
    width: usize,
    height: usize,
    transform: &lumos::TransformMatrix,
) -> AstroImage {
    let channels = image.channels();

    // Warp each channel separately
    let mut warped_pixels = Vec::with_capacity(image.pixels().len());

    for c in 0..channels {
        // Extract channel
        let channel: Vec<f32> = image
            .pixels()
            .iter()
            .skip(c)
            .step_by(channels)
            .copied()
            .collect();

        // Warp channel
        let warped_channel = lumos::registration::warp_to_reference(
            &channel,
            width,
            height,
            transform,
            InterpolationMethod::Lanczos3,
        );

        // Interleave back
        if c == 0 {
            warped_pixels.resize(image.pixels().len(), 0.0);
        }
        for (i, &val) in warped_channel.iter().enumerate() {
            warped_pixels[i * channels + c] = val;
        }
    }

    let dims = image.dimensions();
    let mut result = AstroImage::from_pixels(dims.width, dims.height, dims.channels, warped_pixels);
    result.metadata = image.metadata.clone();
    result
}

/// Save a visualization of detected stars on the reference image.
fn save_star_visualization(ref_image: &AstroImage, stars: &[Star], output_dir: &Path) {
    // Filter out stars in the bottom portion of the image (horizon, ground)
    let height_cutoff = ref_image.dimensions().height as f32 * 0.7;
    let sky_stars: Vec<&Star> = stars.iter().filter(|s| s.y < height_cutoff).collect();

    // Take the best stars
    let best_stars: Vec<&Star> = sky_stars.iter().copied().take(MAX_STARS_TO_DRAW).collect();

    // Print info about the brightest stars
    println!("\nTop 10 brightest stars in reference frame:");
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
    let mut output_image = ref_image.clone();
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
        "Star visualization saved"
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
    let width = image.width();
    let height = image.height();
    let channels = image.channels();

    let min_radius = (radius - thickness as f32 / 2.0).max(1.0);
    let max_radius = radius + thickness as f32 / 2.0;
    let min_r_sq = min_radius * min_radius;
    let max_r_sq = max_radius * max_radius;

    // Bounding box for the circle
    let x_min = ((cx - max_radius).floor() as i32).max(0) as usize;
    let x_max = ((cx + max_radius).ceil() as i32).min(width as i32 - 1) as usize;
    let y_min = ((cy - max_radius).floor() as i32).max(0) as usize;
    let y_max = ((cy + max_radius).ceil() as i32).min(height as i32 - 1) as usize;

    let pixels = image.pixels_mut();
    for y in y_min..=y_max {
        for x in x_min..=x_max {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let dist_sq = dx * dx + dy * dy;

            if dist_sq >= min_r_sq && dist_sq <= max_r_sq {
                let idx = (y * width + x) * channels;
                if channels == 1 {
                    pixels[idx] = 0.299 * color[0] + 0.587 * color[1] + 0.114 * color[2];
                } else {
                    pixels[idx] = color[0];
                    pixels[idx + 1] = color[1];
                    pixels[idx + 2] = color[2];
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
