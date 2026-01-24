use criterion::{criterion_group, criterion_main};
use lumos::bench::calibration_dir;

fn median_benchmarks(c: &mut criterion::Criterion) {
    let calibration_dir =
        calibration_dir().expect("LUMOS_CALIBRATION_DIR must be set with Darks subdirectory");
    lumos::bench::median::benchmarks(c, &calibration_dir);
}

criterion_group!(benches, median_benchmarks);
criterion_main!(benches);
