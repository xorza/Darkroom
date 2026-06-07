//! Example: full astrophotography pipeline — raw lights to a stacked master.
//!
//! Two public steps:
//! 1. Build calibration masters from raw dark/flat/bias frames.
//! 2. [`calibrate_align_stack`] — load each raw light → calibrate → demosaic → detect →
//!    register → warp → stack, in a single call.
//!
//! Reads `Darks/`, `Flats/`, `Bias/`, `Lights/` subdirectories of raw frames from the bundled
//! dataset (`test_data/lumos_data`). The stacked master is written to `test_output/`.
//!
//! ```bash
//! cargo run --release --example full_pipeline
//! ```

use std::path::{Path, PathBuf};
use std::time::Instant;

use lumos::{
    AlignStackConfig, CalibrationFrames, CalibrationMasters, DEFAULT_SIGMA_THRESHOLD,
    calibrate_align_stack,
};
use tracing_subscriber::EnvFilter;

fn main() {
    init_tracing();
    let start = Instant::now();

    let calibration_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test_data/lumos_data");
    assert!(
        calibration_dir.is_dir(),
        "bundled dataset not found at {}",
        calibration_dir.display()
    );
    tracing::info!(path = %calibration_dir.display(), "Calibration directory");

    // Step 1 — calibration masters from raw dark/flat/bias frames.
    let masters = create_calibration_masters(&calibration_dir);

    // Step 2 — raw lights → calibrated, registered, stacked master, in one call.
    let light_paths = common::file_utils::astro_image_files(&calibration_dir.join("Lights"));
    assert!(!light_paths.is_empty(), "no light frames found in Lights/");
    tracing::info!(count = light_paths.len(), "Light frames");

    let result = calibrate_align_stack(&light_paths, &masters, &AlignStackConfig::default())
        .expect("calibrate_align_stack failed");

    tracing::info!(
        "Stacked {} of {} lights (reference frame #{}, dropped {:?})",
        result.registered,
        light_paths.len(),
        result.reference,
        result.dropped,
    );

    // Save the stacked master.
    let output = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("test_output/stacked_result.tiff");
    std::fs::create_dir_all(output.parent().unwrap()).expect("create output directory");
    let image: imaginarium::Image = result.image.into();
    image.save_file(&output).expect("save stacked master");

    println!(
        "\nPipeline complete in {:.1}s — saved {}",
        start.elapsed().as_secs_f32(),
        output.display()
    );
}

/// Build calibration masters from raw frames in `Darks/`, `Flats/`, and `Bias/`.
fn create_calibration_masters(calibration_dir: &Path) -> CalibrationMasters {
    let load = |subdir: &str| -> Vec<PathBuf> {
        let dir = calibration_dir.join(subdir);
        if dir.exists() {
            common::file_utils::astro_image_files(&dir)
        } else {
            Vec::new()
        }
    };
    let darks = load("Darks");
    let flats = load("Flats");
    let bias = load("Bias");
    tracing::info!(
        darks = darks.len(),
        flats = flats.len(),
        bias = bias.len(),
        "Calibration frames"
    );

    let empty: Vec<PathBuf> = Vec::new();
    let masters = CalibrationMasters::from_files(
        CalibrationFrames {
            darks: &darks,
            flats: &flats,
            bias: &bias,
            flat_darks: &empty,
        },
        DEFAULT_SIGMA_THRESHOLD,
    )
    .expect("failed to build calibration masters");

    if let Some(ref defects) = masters.defect_map {
        tracing::info!(count = defects.count(), "Hot/cold pixels detected");
    }
    masters
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
}
