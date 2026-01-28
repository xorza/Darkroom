use criterion::{Bencher, Criterion, criterion_group, criterion_main};
use lumos::bench::{BackgroundMap, create_threshold_mask, detection_scalar as scalar};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

fn bench_create_threshold_mask(c: &mut Criterion) {
    let mut group = c.benchmark_group("create_threshold_mask");

    let width = 2048;
    let height = 2048;
    let pixel_count = width * height;
    let sigma_threshold = 5.0;

    let mut rng = StdRng::seed_from_u64(42);
    let pixels: Box<[f32]> = (0..pixel_count)
        .map(|_| rng.random::<f32>() * 100.0f32)
        .collect::<Vec<_>>()
        .into_boxed_slice();
    let background = BackgroundMap {
        background: (0..pixel_count)
            .map(|_| rng.random::<f32>() * 10.0 + 50.0)
            .collect(),
        noise: (0..pixel_count)
            .map(|_| rng.random::<f32>() * 2.0 + 1.0)
            .collect(),
        width,
        height,
    };

    let setup = || (pixels.clone(), background.clone());

    group.bench_function("dispatch", |b: &mut Bencher| {
        b.iter_batched(
            setup,
            |(pixels, bg)| {
                let mut mask = Vec::with_capacity(pixels.len());
                create_threshold_mask(&pixels, &bg, sigma_threshold, &mut mask);
                mask
            },
            criterion::BatchSize::LargeInput,
        )
    });

    group.bench_function("scalar", |b: &mut Bencher| {
        b.iter_batched(
            setup,
            |(pixels, bg)| {
                let mut mask = Vec::with_capacity(pixels.len());
                scalar::create_threshold_mask(&pixels, &bg, sigma_threshold, &mut mask);
                mask
            },
            criterion::BatchSize::LargeInput,
        )
    });

    group.finish();
}

criterion_group!(benches, bench_create_threshold_mask);
criterion_main!(benches);
