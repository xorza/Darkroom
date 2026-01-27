//! Benchmark for math operations.

use criterion::{Criterion, criterion_group, criterion_main};

fn benchmarks(c: &mut Criterion) {
    lumos::math::bench::benchmarks(c);
}

criterion_group!(benches, benchmarks);
criterion_main!(benches);
