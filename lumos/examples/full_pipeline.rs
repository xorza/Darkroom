//! Example: full astrophotography pipeline — raw lights to a stacked master.
//!
//! Two public steps:
//! 1. Build calibration masters from raw dark/flat/bias frames.
//! 2. [`AlignStackConfig::calibrate_align_stack_paths`] — load each raw light → calibrate →
//!    demosaic → detect → register → warp → stack, in a single call.
//!
//! Reads `Darks/`, `Flats/`, `Bias/`, `Lights/` subdirectories of raw frames from the bundled
//! dataset (`test_data/lumos_data`). The stacked master is written to `test_output/`.
//!
//! ```bash
//! cargo run --profile release-dev --example full_pipeline
//! ```

#![expect(clippy::print_stdout, reason = "an example reports to the terminal")]

use std::path::{Path, PathBuf};
use std::time::Instant;

use common::{fs, internals};
use lumos::{
    AlignStackConfig, CalibratedLights, CalibrationMasters, CalibrationSet, IngestConfig,
    MasterBuild, NoMasterCache, ProgressMonitor, SigmaThreshold,
};
use tracing_subscriber::EnvFilter;

fn main() {
    init_tracing();
    let start = Instant::now();

    let calibration_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test_data/lumos_data");
    assert!(
        fs::is_dir(&calibration_dir),
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
    // (`AlignStackConfig::calibrate_align_stack_paths` narrates its own load → detect → register →
    // stack phases.)
    let light_paths =
        lumos::raw_files(&calibration_dir.join("Lights")).expect("scan raw light frames");
    assert!(!light_paths.is_empty(), "no light frames found in Lights/");
    tracing::info!(
        lights = light_paths.len(),
        "── Step 2/2: calibrate → align → stack ──"
    );
    let step = Instant::now();
    let result = AlignStackConfig::default()
        .calibrate_align_stack_paths(
            CalibratedLights {
                paths: &light_paths,
                masters: &masters,
            },
            &IngestConfig::default(),
            ProgressMonitor::unattended(),
        )
        .expect("calibrate_align_stack failed");
    tracing::info!(
        registered = result.registered(),
        total = light_paths.len(),
        reference = result.reference,
        dropped = ?result.dropped(),
        width = result.product.image.width(),
        height = result.product.image.height(),
        elapsed = elapsed(step),
        "Step 2 complete: stacked master ready"
    );

    // Save the stacked master.
    let output = internals::output_path("stacked_result.tiff");
    result
        .product
        .image
        .write_raster(&output)
        .expect("save stacked master");
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
        if fs::exists(&dir) {
            lumos::raw_files(&dir).expect("scan raw calibration frames")
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

    // Each role stacks under its own preset, the flats with the bias taken from every frame; the
    // set is assembled once every master exists.
    let masters = CalibrationMasters::build(
        MasterBuild {
            frames: CalibrationSet {
                dark: darks.as_slice(),
                flat: flats.as_slice(),
                bias: bias.as_slice(),
                flat_dark: &[],
            },
            sigma_threshold: SigmaThreshold::DEFAULT,
            cache: &NoMasterCache,
        },
        &IngestConfig::default(),
        ProgressMonitor::unattended(),
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
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .try_init()
        .ok();
}
