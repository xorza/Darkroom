use criterion::{criterion_group, criterion_main};
use lumos::bench::first_raw_file;

fn demosaic_benchmarks(c: &mut criterion::Criterion) {
    let raw_file = first_raw_file().expect("LUMOS_CALIBRATION_DIR must be set with Lights subdir");
    lumos::bench::demosaic::benchmarks(c, &raw_file);
}

criterion_group!(benches, demosaic_benchmarks);
criterion_main!(benches);
