//! Benchmark for registration operations.

use std::path::PathBuf;

use criterion::{Criterion, criterion_group, criterion_main};

fn benchmarks(c: &mut Criterion) {
    let calibration_dir = PathBuf::from(std::env::var("LUMOS_CALIBRATION_DIR").unwrap_or_default());
    lumos::registration::bench::benchmarks(c, &calibration_dir);
}

criterion_group!(benches, benchmarks);
criterion_main!(benches);
