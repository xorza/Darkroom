//! Example: Full astrophotography calibration pipeline
//!
//! This example demonstrates the complete calibration workflow:
//! 1. Create master calibration frames (darks, flats, biases) from raw frames
//! 2. Calibrate light frames using the master frames
//! 3. Detect stars in calibrated images
//! 4. Register (align) all lights to a reference frame
//! 5. Stack aligned lights into final image
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
//!   stacked_result.tiff
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
    AstroImage, CalibrationMasters, FilteringConfig, FrameType, ImageStack, InterpolationMethod,
    MedianConfig, ProgressCallback, RegistrationConfig, Registrator, SigmaClippedConfig,
    StackingMethod, StackingProgress, StackingStage, Star, StarDetectionConfig, StarDetector,
    TransformType,
};
use tracing_subscriber::EnvFilter;

/// Maximum stars to use for registration.
const MAX_STARS_FOR_REGISTRATION: usize = 500;

fn main() {
    init_tracing();

    let total_start = Instant::now();

    // Get calibration directory from environment
    let calibration_dir = env::var("LUMOS_CALIBRATION_DIR")
        .map(PathBuf::from)
        .expect("LUMOS_CALIBRATION_DIR environment variable must be set");

    tracing::info!(path = %calibration_dir.display(), "Calibration directory");

    // Set up output directories
    let output_base = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("test_output");
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

    let registered_paths =
        register_all_lights(&calibrated_paths, &stars_per_image, &registered_dir);

    // =========================================================================
    // STEP 5: Stack aligned lights into final image
    // =========================================================================
    println!("\n{}", "=".repeat(60));
    println!("STEP 5: Stacking aligned lights into final image");
    println!("{}\n", "=".repeat(60));

    stack_registered_lights(&registered_paths, &output_base);

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
    if let Ok(masters) = CalibrationMasters::load(output_dir, method.clone())
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

    let masters = CalibrationMasters::create(calibration_dir, method, progress)
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
        // Check if we already have calibrated lights from a previous run
        let existing_calibrated: Vec<PathBuf> = std::fs::read_dir(output_dir)
            .ok()
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .map(|e| e.path())
                    .filter(|p| {
                        p.extension().is_some_and(|ext| ext == "tiff")
                            && p.file_name()
                                .is_some_and(|n| n.to_string_lossy().contains("_calibrated"))
                    })
                    .collect()
            })
            .unwrap_or_default();
        if !existing_calibrated.is_empty() {
            tracing::info!(
                "Found {} existing calibrated lights in output directory",
                existing_calibrated.len()
            );
            return existing_calibrated;
        }
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
        filtering: FilteringConfig {
            edge_margin: 20,
            min_snr: 10.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut detector = StarDetector::from_config(config);

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

        let result = detector.detect(&image);

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
/// Returns paths to successfully registered images.
fn register_all_lights(
    calibrated_paths: &[PathBuf],
    stars_per_image: &[Vec<Star>],
    output_dir: &Path,
) -> Vec<PathBuf> {
    if calibrated_paths.is_empty() {
        tracing::warn!("No calibrated images to register");
        return Vec::new();
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

    // Load reference image
    let ref_image = AstroImage::from_file(ref_path).expect("Failed to load reference image");

    // Configure registration for high accuracy
    let reg_config = RegistrationConfig {
        transform_type: TransformType::Homography, // Can model distortions that similarity cannot
        ransac_iterations: 5000,                   // More iterations for better model
        ransac_threshold: 1.0,                     // Tighter threshold = stricter inlier selection
        ransac_confidence: 0.9999,                 // Higher confidence
        max_stars_for_matching: MAX_STARS_FOR_REGISTRATION,
        min_matched_stars: 20,       // Require more matched stars
        max_residual_pixels: 1.0,    // Stricter residual limit
        triangle_tolerance: 0.005,   // Tighter triangle matching
        refine_with_centroids: true, // Enable centroid refinement
        ..Default::default()
    };

    let registrator = Registrator::new(reg_config);

    let mut registered_paths = Vec::new();
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
            // Reference frame - just copy as-is
            let img: imaginarium::Image = ref_image.clone().into();
            img.save_file(&output_path)
                .expect("Failed to save reference frame");
            tracing::info!(file = %filename, "Reference frame saved");
            registered_paths.push(output_path);
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

        // Register target to reference
        match registrator.register_stars(ref_stars, stars) {
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
                let warped = lumos::warp_to_reference_image(
                    &target_image,
                    &result.transform,
                    InterpolationMethod::Lanczos3,
                );

                // Save registered image
                let img: imaginarium::Image = warped.into();
                img.save_file(&output_path)
                    .expect("Failed to save registered frame");

                registered_paths.push(output_path);
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
        successful = registered_paths.len(),
        failed = failed_registrations,
        elapsed_secs = format!("{:.2}", elapsed.as_secs_f32()),
        "Step 4 complete: All lights registered"
    );

    registered_paths
}

/// Step 5: Stack all registered lights into a final image.
fn stack_registered_lights(registered_paths: &[PathBuf], output_dir: &Path) {
    if registered_paths.is_empty() {
        tracing::warn!("No registered images to stack");
        return;
    }

    if registered_paths.len() < 2 {
        tracing::warn!(
            "Need at least 2 images to stack, only have {}",
            registered_paths.len()
        );
        return;
    }

    let start = Instant::now();

    // Use sigma-clipped mean for best outlier rejection (cosmic rays, satellites, etc.)
    let stacking_config = SigmaClippedConfig::default();
    let method = StackingMethod::SigmaClippedMean(stacking_config);

    tracing::info!(
        image_count = registered_paths.len(),
        method = %method,
        "Starting final stack"
    );

    let stack = ImageStack::new(FrameType::Light, method, registered_paths.to_vec());
    let progress = create_progress_callback();

    match stack.process(progress) {
        Ok(stacked) => {
            let output_path = output_dir.join("stacked_result.tiff");

            // Save the stacked result
            let img: imaginarium::Image = stacked.into();
            img.save_file(&output_path)
                .expect("Failed to save stacked result");

            let elapsed = start.elapsed();
            tracing::info!(
                output = %output_path.display(),
                elapsed_secs = format!("{:.2}", elapsed.as_secs_f32()),
                "Step 5 complete: Final stack saved"
            );
        }
        Err(e) => {
            tracing::error!(error = %e, "Stacking failed");
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
