//! Benchmark for star detection algorithms.
//!
//! Run with: cargo bench -p lumos --features bench --bench star_detection_detection

use criterion::{Criterion, criterion_group, criterion_main};

fn detection_benchmarks(c: &mut Criterion) {
    lumos::bench::detection::benchmarks(c);
}

criterion_group!(benches, detection_benchmarks);
criterion_main!(benches);
