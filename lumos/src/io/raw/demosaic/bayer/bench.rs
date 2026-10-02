//! Benchmarks for the Bayer demosaic.

use common::CancelToken;
use quickbench::quick_bench;

use crate::io::raw::demosaic::bayer::{BayerImage, CfaPattern, rcd};
use crate::io::raw::demosaic::sensor_layout::SensorLayout;
use crate::math::size2us::Size2us;

/// The RCD demosaic core alone on a synthetic Bayer gradient, with no file loading or
/// normalization.
#[quick_bench(warmup_iters = 1, iters = 5)]
fn bench_rcd_demosaic_core(b: quickbench::Bencher) {
    for (w, h) in [(1000, 1000), (4000, 3000), (6000, 4000)] {
        let data: Vec<f32> = (0..w * h)
            .map(|i| (((i % w) + (i / w)) as f32 / (w + h) as f32).min(1.0))
            .collect();
        let size = Size2us::new(w, h);
        let bayer = BayerImage::with_margins(&data, SensorLayout::cropped(size), CfaPattern::Rggb);
        b.bench_labeled(&format!("{w}x{h}"), || {
            rcd::demosaic(&bayer, &CancelToken::never()).unwrap()
        });
    }
}
