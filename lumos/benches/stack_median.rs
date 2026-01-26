use criterion::{criterion_group, criterion_main};
use lumos::bench::calibration_dir;

fn median_benchmarks(c: &mut criterion::Criterion) {
    lumos::bench::median::benchmarks(c);
}

criterion_group!(benches, median_benchmarks);
criterion_main!(benches);
