//! Example: Create calibration master frames (flats, biases, darks)
//!
//! This example reads calibration frames from LUMOS_CALIBRATION_DIR and creates
//! master frames using median stacking. The masters are saved to test_output/calibration_masters.
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
//! ```
//!
//! Output structure:
//! ```text
//! test_output/calibration_masters/
//!   master_dark_median.tiff
//!   master_flat_median.tiff
//!   master_bias_median.tiff
//! ```
//!
//! # Usage
//!
//! ```bash
//! LUMOS_CALIBRATION_DIR=/path/to/calibration cargo run --example create_calibration_masters
//! ```

use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use lumos::{
    CalibrationMasters, MedianConfig, ProgressCallback, StackingMethod, StackingProgress,
    StackingStage,
};
use tracing_subscriber::EnvFilter;

fn main() {
    // Initialize tracing for console output
    init_tracing();

    // Get calibration directory from environment
    let calibration_dir = env::var("LUMOS_CALIBRATION_DIR")
        .map(PathBuf::from)
        .expect("LUMOS_CALIBRATION_DIR environment variable must be set");

    tracing::info!(path = %calibration_dir.display(), "Calibration directory");

    // Create output directory
    let output_dir = PathBuf::from("test_output/calibration_masters");
    fs::create_dir_all(&output_dir).expect("Failed to create output directory");
    tracing::info!(path = %output_dir.display(), "Output directory");

    // Create calibration masters from directory
    let start = Instant::now();
    let config = MedianConfig::default();
    let method = StackingMethod::Median(config);
    let progress = create_progress_callback();

    tracing::info!("Creating calibration masters...");

    let masters = CalibrationMasters::from_directory(&calibration_dir, method, progress)
        .expect("Failed to create calibration masters");

    let elapsed = start.elapsed();

    // Report what was created
    if let Some(ref dark) = masters.master_dark {
        tracing::info!(
            width = dark.dimensions.width,
            height = dark.dimensions.height,
            channels = dark.dimensions.channels,
            "Master dark created"
        );
    } else {
        tracing::warn!("No master dark created (no Darks subdirectory or frames)");
    }

    if let Some(ref flat) = masters.master_flat {
        tracing::info!(
            width = flat.dimensions.width,
            height = flat.dimensions.height,
            channels = flat.dimensions.channels,
            "Master flat created"
        );
    } else {
        tracing::warn!("No master flat created (no Flats subdirectory or frames)");
    }

    if let Some(ref bias) = masters.master_bias {
        tracing::info!(
            width = bias.dimensions.width,
            height = bias.dimensions.height,
            channels = bias.dimensions.channels,
            "Master bias created"
        );
    } else {
        tracing::warn!("No master bias created (no Bias subdirectory or frames)");
    }

    if let Some(ref hot_pixels) = masters.hot_pixel_map {
        tracing::info!(
            count = hot_pixels.count,
            percentage = format!("{:.4}%", hot_pixels.percentage()),
            "Hot pixels detected from master dark"
        );
    }

    // Save masters to output directory
    masters
        .save_to_directory(&output_dir)
        .expect("Failed to save calibration masters");

    // Summary
    tracing::info!(
        elapsed_secs = elapsed.as_secs_f32(),
        output_dir = %output_dir.display(),
        "Calibration masters complete"
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
