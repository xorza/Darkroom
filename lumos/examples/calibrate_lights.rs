//! Example: Calibrate light frames using master calibration frames
//!
//! This example reads light frames from LUMOS_CALIBRATION_DIR/Lights and calibrates them
//! using master frames from LUMOS_CALIBRATION_DIR/calibration_masters. The calibrated
//! lights are saved to test_output/calibrated_lights.
//!
//! # Directory Structure
//!
//! Expected input structure:
//! ```text
//! $LUMOS_CALIBRATION_DIR/
//!   Lights/
//!     light1.raf, light2.raf, ...
//!   calibration_masters/
//!     master_dark_median.tiff
//!     master_flat_median.tiff
//!     master_bias_median.tiff
//! ```
//!
//! Output structure:
//! ```text
//! test_output/calibrated_lights/
//!   light1_calibrated.tiff
//!   light2_calibrated.tiff
//!   ...
//! ```
//!
//! # Usage
//!
//! ```bash
//! LUMOS_CALIBRATION_DIR=/path/to/calibration cargo run --example calibrate_lights
//! ```

use std::env;
use std::fs;
use std::path::PathBuf;
use std::time::Instant;

use lumos::{AstroImage, CalibrationMasters, StackingMethod};
use tracing_subscriber::EnvFilter;

fn main() {
    // Initialize tracing for console output
    init_tracing();

    // Get calibration directory from environment
    let calibration_dir = env::var("LUMOS_CALIBRATION_DIR")
        .map(PathBuf::from)
        .expect("LUMOS_CALIBRATION_DIR environment variable must be set");

    tracing::info!(path = %calibration_dir.display(), "Calibration directory");

    // Set up paths
    let lights_dir = calibration_dir.join("Lights");
    let masters_dir = calibration_dir.join("calibration_masters");
    let output_dir = PathBuf::from("test_output/calibrated_lights");

    // Verify directories exist
    assert!(
        lights_dir.exists(),
        "Lights directory does not exist: {}",
        lights_dir.display()
    );
    assert!(
        masters_dir.exists(),
        "Calibration masters directory does not exist: {}. Run create_calibration_masters example first.",
        masters_dir.display()
    );

    // Create output directory
    fs::create_dir_all(&output_dir).expect("Failed to create output directory");
    tracing::info!(path = %output_dir.display(), "Output directory");

    // Load calibration masters
    tracing::info!(path = %masters_dir.display(), "Loading calibration masters");
    let masters = CalibrationMasters::load_from_directory(&masters_dir, StackingMethod::default())
        .expect("Failed to load calibration masters");

    // Report loaded masters
    if masters.master_dark.is_some() {
        tracing::info!("Loaded master dark");
    }
    if masters.master_flat.is_some() {
        tracing::info!("Loaded master flat");
    }
    if masters.master_bias.is_some() {
        tracing::info!("Loaded master bias");
    }
    if let Some(ref hot_pixels) = masters.hot_pixel_map {
        tracing::info!(
            count = hot_pixels.count,
            percentage = format!("{:.4}%", hot_pixels.percentage()),
            "Hot pixel map loaded"
        );
    }

    // Find light frames
    let light_paths = common::file_utils::astro_image_files(&lights_dir);
    if light_paths.is_empty() {
        tracing::error!("No light frames found in {}", lights_dir.display());
        return;
    }
    tracing::info!(count = light_paths.len(), "Found light frames");

    // Calibrate each light frame in parallel
    let start = Instant::now();
    let results: Vec<_> = light_paths
        .iter()
        .map(|path| {
            let filename = path.file_stem().unwrap().to_string_lossy();

            tracing::info!(file = %filename, "Calibrating");

            // Load the light frame
            let mut light = match AstroImage::from_file(path) {
                Ok(img) => img,
                Err(e) => {
                    tracing::error!(file = %filename, error = %e, "Failed to load");
                    return Err::<PathBuf, _>(e.to_string());
                }
            };

            // Calibrate
            masters.calibrate(&mut light);

            // Save calibrated frame
            let output_path = output_dir.join(format!("{}_calibrated.tiff", filename));
            let img: imaginarium::Image = light.into();
            if let Err(e) = img.save_file(&output_path) {
                tracing::error!(file = %filename, error = %e, "Failed to save");
                return Err(e.to_string());
            }

            tracing::info!(file = %filename, "Calibrated and saved");
            Ok(output_path)
        })
        .collect();

    let elapsed = start.elapsed();

    // Summary
    let successful = results.iter().filter(|r| r.is_ok()).count();
    let failed = results.iter().filter(|r| r.is_err()).count();

    tracing::info!(
        successful,
        failed,
        elapsed_secs = elapsed.as_secs_f32(),
        "Calibration complete"
    );

    // List output files
    println!("\nOutput files:");
    for entry in fs::read_dir(&output_dir).expect("Failed to read output directory") {
        let entry = entry.expect("Failed to read directory entry");
        let metadata = entry.metadata().expect("Failed to get file metadata");
        let size_mb = metadata.len() as f64 / (1024.0 * 1024.0);
        println!(
            "  {} ({:.1} MB)",
            entry.file_name().to_string_lossy(),
            size_mb
        );
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
