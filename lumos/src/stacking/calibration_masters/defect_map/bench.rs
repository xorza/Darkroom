use crate::io::raw::demosaic::bayer::CfaPattern;
use crate::stacking::calibration_masters::defect_map::*;
use ::quickbench::quick_bench;

#[quick_bench(warmup_iters = 3, iters = 20)]
fn bench_collect_color_samples(b: quickbench::Bencher) {
    let (w, h) = (6000, 4000);
    let data = Buffer2::new(w, h, (0..w * h).map(|i| (i % 1000) as f32).collect());
    let cfa = CfaType::Bayer(CfaPattern::Rggb);
    b.bench(|| {
        std::hint::black_box(collect_color_samples(
            std::hint::black_box(&data),
            Some(&cfa),
            0,
        ))
    });
}
