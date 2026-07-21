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

use common::{CancelToken, file_utils};
use lumos::{
    AlignStackConfig, CalibrationMasters, CalibrationSet, DEFAULT_SIGMA_THRESHOLD, RAW_EXTENSIONS,
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
    tracing::info!("── Step 1/2: building calibration masters ──");
    let step = Instant::now();
    let masters = create_calibration_masters(&calibration_dir);
    tracing::info!(elapsed = elapsed(step), "Step 1 complete: masters ready");

    // Step 2 — raw lights → calibrated, registered, stacked master, in one call.
    // (`calibrate_align_stack` narrates its own load → detect → register → stack phases.)
    let light_paths =
        file_utils::files_with_extensions(&calibration_dir.join("Lights"), RAW_EXTENSIONS)
            .expect("scan raw light frames");
    assert!(!light_paths.is_empty(), "no light frames found in Lights/");
    tracing::info!(
        lights = light_paths.len(),
        "── Step 2/2: calibrate → align → stack ──"
    );
    let step = Instant::now();
    let result = calibrate_align_stack(
        &light_paths,
        &masters,
        &AlignStackConfig::default(),
        CancelToken::never(),
    )
    .expect("calibrate_align_stack failed");
    tracing::info!(
        registered = result.alignment.registered,
        total = light_paths.len(),
        reference = result.alignment.reference,
        dropped = ?result.alignment.dropped,
        width = result.product.image.width(),
        height = result.product.image.height(),
        elapsed = elapsed(step),
        "Step 2 complete: stacked master ready"
    );

    // Save the stacked master.
    let output = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("test_output/stacked_result.tiff");
    std::fs::create_dir_all(output.parent().unwrap()).expect("create output directory");
    let image: imaginarium::Image = result.product.image.into();
    image.save_file(&output).expect("save stacked master");
    tracing::info!(path = %output.display(), "Saved stacked master");

    println!(
        "\nPipeline complete in {:.1}s — saved {}",
        start.elapsed().as_secs_f32(),
        output.display()
    );
}

/// Seconds elapsed since `since`, formatted for a tracing field (e.g. `"12.3s"`).
fn elapsed(since: Instant) -> String {
    format!("{:.1}s", since.elapsed().as_secs_f32())
}

/// Build calibration masters from raw frames in `Darks/`, `Flats/`, and `Bias/`.
fn create_calibration_masters(calibration_dir: &Path) -> CalibrationMasters {
    let load = |subdir: &str| -> Vec<PathBuf> {
        let dir = calibration_dir.join(subdir);
        if dir.exists() {
            file_utils::files_with_extensions(&dir, RAW_EXTENSIONS)
                .expect("scan raw calibration frames")
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
        CalibrationSet {
            dark: &darks,
            flat: &flats,
            bias: &bias,
            flat_dark: &empty,
        },
        DEFAULT_SIGMA_THRESHOLD,
        CancelToken::never(),
    )
    .expect("failed to build calibration masters");

    if let Some(defects) = masters.defect_summary() {
        tracing::info!(
            hot = defects.hot_pixels,
            cold = defects.cold_pixels,
            "Defect map built (hot from dark, cold/dead from flat)"
        );
    }
    masters
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
}
