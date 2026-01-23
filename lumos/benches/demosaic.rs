use criterion::{criterion_group, criterion_main};
use std::path::PathBuf;

/// Returns the calibration directory from LUMOS_CALIBRATION_DIR env var.
fn calibration_dir() -> Option<PathBuf> {
    std::env::var("LUMOS_CALIBRATION_DIR")
        .ok()
        .map(PathBuf::from)
}

/// Returns the first RAW file from the Lights subdirectory.
fn first_raw_file() -> Option<PathBuf> {
    let cal_dir = calibration_dir()?;
    let lights = cal_dir.join("Lights");
    if !lights.exists() {
        return None;
    }
    common::file_utils::astro_image_files(&lights)
        .first()
        .cloned()
}

fn demosaic_benchmarks(c: &mut criterion::Criterion) {
    let raw_file = first_raw_file().expect("LUMOS_CALIBRATION_DIR must be set with Lights subdir");
    lumos::bench::demosaic::benchmarks(c, &raw_file);
}

criterion_group!(benches, demosaic_benchmarks);
criterion_main!(benches);
