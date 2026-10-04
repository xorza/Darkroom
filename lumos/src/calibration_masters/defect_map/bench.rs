use crate::calibration_masters::defect_map::sampling::collect_color_sample_indices;
use crate::calibration_masters::defect_map::*;
use crate::io::raw::demosaic::bayer::CfaPattern;
use ::quickbench::quick_bench;
use std::hint;

#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_collect_color_sample_indices(b: quickbench::Bencher) {
    let size = Size2us::new(6000, 4000);
    let cfa = CfaType::Bayer(CfaPattern::Rggb);
    b.bench(|| hint::black_box(collect_color_sample_indices(hint::black_box(size), cfa, 0)));
}
