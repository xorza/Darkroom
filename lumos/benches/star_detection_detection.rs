use criterion::{criterion_group, criterion_main};

fn detection_benchmarks(c: &mut criterion::Criterion) {
    lumos::bench::detection::benchmarks(c);
}

criterion_group!(benches, detection_benchmarks);
criterion_main!(benches);
