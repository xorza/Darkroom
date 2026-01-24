use criterion::{criterion_group, criterion_main};
use lumos::bench::calibration_masters_dir;

fn hot_pixels_benchmarks(c: &mut criterion::Criterion) {
    let masters_dir = calibration_masters_dir()
        .expect("LUMOS_CALIBRATION_DIR must be set with calibration_masters subdir");
    lumos::bench::hot_pixels::benchmarks(c, &masters_dir);
}

criterion_group!(benches, hot_pixels_benchmarks);
criterion_main!(benches);
