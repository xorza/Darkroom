use criterion::{criterion_group, criterion_main};

fn centroid_benchmarks(c: &mut criterion::Criterion) {
    lumos::bench::centroid::benchmarks(c);
}

criterion_group!(benches, centroid_benchmarks);
criterion_main!(benches);
