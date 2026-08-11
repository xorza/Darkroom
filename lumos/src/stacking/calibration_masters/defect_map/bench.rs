use crate::io::raw::demosaic::bayer::CfaPattern;
use crate::stacking::calibration_masters::defect_map::sampling::collect_color_samples;
use crate::stacking::calibration_masters::defect_map::*;
use ::quickbench::quick_bench;

#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_collect_color_samples(b: quickbench::Bencher) {
    let size = Size2us::new(6000, 4000);
    let data = Buffer2::new(
        size.width,
        size.height,
        (0..size.pixel_count()).map(|i| (i % 1000) as f32).collect(),
    );
    let cfa = CfaType::Bayer(CfaPattern::Rggb);
    b.bench(|| {
        std::hint::black_box(collect_color_samples(
            std::hint::black_box(&data),
            Some(&cfa),
            0,
        ))
    });
}
