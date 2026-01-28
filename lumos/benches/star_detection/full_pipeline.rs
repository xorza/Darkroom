//! Benchmark for the full star detection pipeline.
//!
//! Run with: cargo bench -p lumos --features bench --bench star_detection_full_pipeline

use criterion::{Criterion, criterion_group, criterion_main};

fn pipeline_benchmarks(c: &mut Criterion) {
    lumos::bench::full_pipeline::benchmarks(c);
}

criterion_group!(benches, pipeline_benchmarks);
criterion_main!(benches);
