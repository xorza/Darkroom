//! Benchmark for the full astrophotography pipeline.
//!
//! This benchmark loads calibrated lights from LUMOS_CALIBRATION_DIR/calibrated_lights,
//! runs star detection, registration, and final light stacking without saving intermediates.
//!
//! Run with: cargo bench -p lumos --features bench --bench full_pipeline

use criterion::{Criterion, criterion_group, criterion_main};

fn pipeline_benchmarks(c: &mut Criterion) {
    lumos::bench::pipeline::benchmarks(c);
}

criterion_group!(benches, pipeline_benchmarks);
criterion_main!(benches);
