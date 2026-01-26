//! Benchmark for image format conversion operations.

use criterion::{Criterion, criterion_group, criterion_main};

fn benchmarks(c: &mut Criterion) {
    imaginarium::bench::conversion::benchmarks(c);
}

criterion_group!(benches, benchmarks);
criterion_main!(benches);
