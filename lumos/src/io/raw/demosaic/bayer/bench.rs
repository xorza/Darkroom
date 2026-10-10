//! Benchmarks for the Bayer demosaic.

use common::CancelToken;
use quickbench::quick_bench;

use crate::internals::cfa::{XTRANS_PATTERN, make_cfa};
use crate::io::image::cfa::CfaType;
use crate::io::raw::demosaic::bayer::{BayerImage, CfaPattern, rcd};
use crate::io::raw::demosaic::xtrans::markesteijn::MarkesteijnPasses;
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
        let bayer = BayerImage::new(&data, size, CfaPattern::Rggb);
        b.bench_labeled(&format!("{w}x{h}"), || {
            rcd::demosaic(&bayer, &CancelToken::never()).unwrap()
        });
    }
}

/// `CfaImage::demosaic` on a 24 MP Bayer and a 26 MP X-Trans frame under a camera white balance:
/// the kernels with the balance applied as their tiles read, as a calibrated light is demosaiced.
/// Each run takes a fresh copy of the frame, which the demosaic consumes.
#[quick_bench(warmup_iters = 1, iters = 5)]
fn bench_cfa_demosaic_balanced(b: quickbench::Bencher) {
    for (label, size, cfa_type) in [
        (
            "bayer 6000x4000",
            Size2us::new(6000, 4000),
            CfaType::Bayer(CfaPattern::Rggb),
        ),
        (
            "x-trans 6240x4160",
            Size2us::new(6240, 4160),
            CfaType::XTrans(XTRANS_PATTERN),
        ),
    ] {
        let data: Vec<f32> = (0..size.pixel_count())
            .map(|i| {
                (((i % size.width) + (i / size.width)) as f32 / (size.width + size.height) as f32)
                    .min(1.0)
            })
            .collect();
        let mut cfa = make_cfa(size, data, cfa_type);
        cfa.metadata.camera_white_balance = Some([2.13, 1.0, 1.71, 1.0]);
        b.bench_labeled(label, || {
            cfa.clone()
                .demosaic(MarkesteijnPasses::One, &CancelToken::never())
                .unwrap()
        });
    }
}
