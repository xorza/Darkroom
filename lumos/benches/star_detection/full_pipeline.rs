//! Benchmark for the full star detection pipeline.
//!
//! Run with: cargo bench -p lumos --features bench --bench star_detection_full_pipeline
//!
//! To profile with samply (recommended):
//!   cargo build --release -p lumos --features bench --bench star_detection_full_pipeline
//!   samply record ./target/release/deps/star_detection_full_pipeline-* --bench --profile-time 10
//!
//! To profile with perf + flamegraph:
//!   cargo build --release -p lumos --features bench --bench star_detection_full_pipeline
//!   perf record -g ./target/release/deps/star_detection_full_pipeline-* --bench --profile-time 10
//!   perf script | inferno-collapse-perf | inferno-flamegraph > flamegraph.svg

use criterion::{Criterion, criterion_group, criterion_main};

fn pipeline_benchmarks(c: &mut Criterion) {
    lumos::bench::full_pipeline::benchmarks(c);
}

criterion_group!(benches, pipeline_benchmarks);
criterion_main!(benches);
