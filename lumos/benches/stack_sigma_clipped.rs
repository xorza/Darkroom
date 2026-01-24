use criterion::{criterion_group, criterion_main};
use lumos::bench::calibration_dir;

fn sigma_clipped_benchmarks(c: &mut criterion::Criterion) {
    let calibration_dir =
        calibration_dir().expect("LUMOS_CALIBRATION_DIR must be set with Darks subdirectory");
    lumos::bench::sigma_clipped::benchmarks(c, &calibration_dir);
}

criterion_group!(benches, sigma_clipped_benchmarks);
criterion_main!(benches);
