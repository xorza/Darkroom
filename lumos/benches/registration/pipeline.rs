//! Benchmark for registration operations.

use criterion::{Criterion, criterion_group, criterion_main};

fn benchmarks(c: &mut Criterion) {
    lumos::bench::registration::benchmarks(c);
}

criterion_group!(benches, benchmarks);
criterion_main!(benches);
