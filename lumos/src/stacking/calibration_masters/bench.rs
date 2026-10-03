//! Benchmarks for the calibration stage: defect-map build, per-light calibration
//! apply, and single-frame cosmic-ray rejection.
//!
//! The master *combine* itself (stacking bias/dark/flat frames into a master) runs
//! through the same engine benched in `stacking::combine::bench`
//! (`bench_stack_{bias,dark,flat}_*`), so it isn't duplicated here.
//!
//! Run: `cargo test -p lumos --release --features bench calibration_masters::bench -- --ignored --nocapture`

use crate::testing::cfa::XTRANS_PATTERN;
use crate::testing::prelude::*;
use quickbench::quick_bench;
use std::hint::black_box;

use crate::io::raw::demosaic::bayer::CfaPattern;
use crate::stacking::calibration_masters::cosmic_ray::reject_cosmic_rays;
use crate::testing::cfa::make_cfa;
use crate::{
    CalibrationMasters, CalibrationSet, CfaImage, CfaType, CosmicRayConfig,
    DEFAULT_SIGMA_THRESHOLD, DefectMap,
};

/// A realistic APS-C sub-frame size for the per-pixel CFA passes.
const W: usize = 2000;
const H: usize = 1500;

fn bayer() -> CfaType {
    CfaType::Bayer(CfaPattern::Rggb)
}

/// Seeded uniform noise in `[base − amp, base + amp]`, plus ~0.1% `defect`-valued outliers so
/// hot/cold detection has something to flag.
fn cfa_pixels(base: f32, amp: f32, defect: f32, seed: u64) -> Vec<f32> {
    let n = W * H;
    let mut rng = TestRng::new(seed);
    let mut px: Vec<f32> = (0..n)
        .map(|_| base + (rng.next_f32() - 0.5) * 2.0 * amp)
        .collect();
    for _ in 0..(n / 1000) {
        px[(rng.next_f64() * n as f64) as usize] = defect;
    }
    px
}

/// A full master set (dark + flat + bias + defect map) over the bench dimensions.
fn make_masters() -> CalibrationMasters {
    CalibrationMasters::from_images(
        CalibrationSet {
            dark: Some(make_cfa(
                Size2us::new(W, H),
                cfa_pixels(0.02, 0.004, 0.9, 0x11),
                bayer(),
            )),
            flat: Some(make_cfa(
                Size2us::new(W, H),
                cfa_pixels(0.6, 0.05, 0.001, 0x22),
                bayer(),
            )),
            bias: Some(make_cfa(
                Size2us::new(W, H),
                cfa_pixels(0.01, 0.002, 0.5, 0x33),
                bayer(),
            )),
            flat_dark: None,
        },
        DEFAULT_SIGMA_THRESHOLD,
        CancelToken::never(),
    )
    .unwrap()
}

#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_calibrate_apply_bayer(b: ::quickbench::Bencher) {
    let masters = make_masters();
    // A fresh, uncalibrated light per call: `calibrate` asserts the frame isn't already
    // calibrated and mutates in place, so the clone is the realistic per-light cost — each
    // light arrives freshly decoded and owned.
    let light = make_cfa(
        Size2us::new(W, H),
        cfa_pixels(0.3, 0.05, 0.95, 0x44),
        bayer(),
    );
    b.bench(|| {
        let mut frame = light.clone();
        masters.calibrate(&mut frame).unwrap();
        black_box(frame)
    });
}

#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_calibrate_30_lights_bayer(b: ::quickbench::Bencher) {
    let masters = make_masters();
    let light = make_cfa(
        Size2us::new(W, H),
        cfa_pixels(0.3, 0.05, 0.95, 0x44),
        bayer(),
    );
    b.bench(|| {
        for _ in 0..30 {
            let mut frame = light.clone();
            masters.calibrate(&mut frame).unwrap();
            black_box(frame);
        }
    });
}

#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_defect_map_build_bayer(b: ::quickbench::Bencher) {
    let dark = make_cfa(
        Size2us::new(W, H),
        cfa_pixels(0.02, 0.004, 0.9, 0x11),
        bayer(),
    );
    let flat = make_cfa(
        Size2us::new(W, H),
        cfa_pixels(0.6, 0.05, 0.001, 0x22),
        bayer(),
    );
    b.bench(|| {
        black_box(
            DefectMap::new(dark.size())
                .detect_hot(
                    black_box(&dark),
                    DEFAULT_SIGMA_THRESHOLD,
                    &CancelToken::never(),
                )
                .unwrap()
                .detect_cold(black_box(&flat), &CancelToken::never())
                .unwrap(),
        )
    });
}

/// A 1 MP faint-sky frame seeded with sharp single-pixel "cosmic-ray" spikes for L.A.Cosmic.
fn cosmic_ray_frame(cfa: CfaType) -> CfaImage {
    const CR_W: usize = 1024;
    const CR_H: usize = 1024;
    let n = CR_W * CR_H;
    let mut rng = TestRng::new(1);
    let mut px: Vec<f32> = (0..n)
        .map(|_| 0.1 + (rng.next_f32() - 0.5) * 0.01)
        .collect();
    for _ in 0..300 {
        px[(rng.next_f64() * n as f64) as usize] = 0.95;
    }
    make_cfa(Size2us::new(CR_W, CR_H), px, cfa)
}

/// Twelve iterations, not the three most benches here use: a pass over this fixture is ~850 ms and
/// its run-to-run spread is wide enough that a median of three moves by more than any change worth
/// making. The extra samples cost ~7 s against the ~27 s release rebuild that any A/B already pays,
/// so they are close to free.
#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_cosmic_ray_reject_mono(b: ::quickbench::Bencher) {
    let frame = cosmic_ray_frame(CfaType::Mono);
    let config = CosmicRayConfig::default();
    b.bench(|| {
        let mut f = frame.clone();
        black_box(reject_cosmic_rays(&mut f, black_box(&config)).unwrap())
    });
}

/// The X-Trans path, which had no bench while it was the one still recomputing its same-colour
/// neighbour set per pixel — a 13×13 `color_at` sweep plus a distance sort at every pixel of every
/// iteration. It now walks the precomputed per-phase table the defect scan uses.
#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_cosmic_ray_reject_xtrans(b: ::quickbench::Bencher) {
    let frame = cosmic_ray_frame(CfaType::XTrans(XTRANS_PATTERN));
    let config = CosmicRayConfig::default();
    b.bench(|| {
        let mut f = frame.clone();
        black_box(reject_cosmic_rays(&mut f, black_box(&config)).unwrap())
    });
}

#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_cosmic_ray_reject_bayer(b: ::quickbench::Bencher) {
    let frame = cosmic_ray_frame(bayer());
    let config = CosmicRayConfig::default();
    b.bench(|| {
        let mut f = frame.clone();
        black_box(reject_cosmic_rays(&mut f, black_box(&config)).unwrap())
    });
}
