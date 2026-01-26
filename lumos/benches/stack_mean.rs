use criterion::{criterion_group, criterion_main};
use lumos::bench::calibration_dir;

fn mean_benchmarks(c: &mut criterion::Criterion) {
    lumos::bench::mean::benchmarks(c);
}

criterion_group!(benches, mean_benchmarks);
criterion_main!(benches);
