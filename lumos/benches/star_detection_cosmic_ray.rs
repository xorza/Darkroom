use criterion::{criterion_group, criterion_main};

fn cosmic_ray_benchmarks(c: &mut criterion::Criterion) {
    lumos::bench::cosmic_ray::benchmarks(c);
}

criterion_group!(benches, cosmic_ray_benchmarks);
criterion_main!(benches);
