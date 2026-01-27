use criterion::{criterion_group, criterion_main};

fn gpu_sigma_clip_benchmarks(c: &mut criterion::Criterion) {
    lumos::bench::gpu::benchmarks(c);
}

criterion_group!(benches, gpu_sigma_clip_benchmarks);
criterion_main!(benches);
