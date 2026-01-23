use criterion::{criterion_group, criterion_main};
use lumos::bench::calibration_dir;

fn xtrans_benchmarks(c: &mut criterion::Criterion) {
    let cal_dir = calibration_dir().expect("LUMOS_CALIBRATION_DIR must be set");
    let lights_dir = cal_dir.join("Siril_Tutorial/Lights");
    let raf_file = common::file_utils::astro_image_files(&lights_dir)
        .into_iter()
        .find(|p| {
            p.extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("raf"))
        })
        .expect("No RAF file found in Siril_Tutorial/Lights");
    lumos::bench::demosaic::xtrans::benchmarks(c, &raf_file);
}

criterion_group!(benches, xtrans_benchmarks);
criterion_main!(benches);
