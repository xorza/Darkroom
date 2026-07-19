//! Benchmarks for the calibration stage: defect-map build, per-light calibration
//! apply, and single-frame cosmic-ray rejection.
//!
//! The master *combine* itself (stacking bias/dark/flat frames into a master) runs
//! through the same engine benched in `stacking::combine::bench`
//! (`bench_stack_{bias,dark,flat}_*`), so it isn't duplicated here.
//!
//! Run: `cargo test -p lumos --release calibration_masters::bench -- --ignored --nocapture`

use common::CancelToken;
use quickbench::quick_bench;
use std::hint::black_box;

use crate::io::raw::demosaic::bayer::CfaPattern;
use crate::stacking::calibration_masters::cosmic_ray::reject_cosmic_rays;
use crate::testing::make_cfa;
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

/// Deterministic pseudo-noise in `[base − amp, base + amp]`, plus ~0.1% `defect`-valued
/// outliers so hot/cold detection has something to flag.
fn cfa_pixels(base: f32, amp: f32, defect: f32, salt: u32) -> Vec<f32> {
    let n = W * H;
    let mut px = vec![0.0f32; n];
    for (i, p) in px.iter_mut().enumerate() {
        let hash = (i as u32).wrapping_mul(2654435761) ^ salt;
        *p = base + (hash as f32 / u32::MAX as f32 - 0.5) * 2.0 * amp;
    }
    for k in 0..(n / 1000) {
        let idx = ((k as u32).wrapping_mul(40503) ^ salt) as usize % n;
        px[idx] = defect;
    }
    px
}

/// A full master set (dark + flat + bias + defect map) over the bench dimensions.
fn make_masters() -> CalibrationMasters {
    CalibrationMasters::from_images(
        CalibrationSet {
            dark: Some(make_cfa(W, H, cfa_pixels(0.02, 0.004, 0.9, 0x11), bayer())),
            flat: Some(make_cfa(W, H, cfa_pixels(0.6, 0.05, 0.001, 0x22), bayer())),
            bias: Some(make_cfa(W, H, cfa_pixels(0.01, 0.002, 0.5, 0x33), bayer())),
            flat_dark: None,
        },
        DEFAULT_SIGMA_THRESHOLD,
        CancelToken::never(),
    )
    .unwrap()
}

#[quick_bench(warmup_iters = 1, iters = 5)]
fn bench_calibrate_apply_bayer(b: ::quickbench::Bencher) {
    let masters = make_masters();
    // A fresh, uncalibrated light per call: `calibrate` asserts the frame isn't already
    // calibrated and mutates in place, so the clone is the realistic per-light cost — each
    // light arrives freshly decoded and owned.
    let light = make_cfa(W, H, cfa_pixels(0.3, 0.05, 0.95, 0x44), bayer());
    b.bench(|| {
        let mut frame = light.clone();
        masters.calibrate(&mut frame).unwrap();
        black_box(frame)
    });
}

#[quick_bench(warmup_iters = 1, iters = 10)]
fn bench_defect_map_build_bayer(b: ::quickbench::Bencher) {
    let dark = make_cfa(W, H, cfa_pixels(0.02, 0.004, 0.9, 0x11), bayer());
    let flat = make_cfa(W, H, cfa_pixels(0.6, 0.05, 0.001, 0x22), bayer());
    b.bench(|| {
        black_box(
            DefectMap::default()
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
    let mut px = vec![0.0f32; n];
    for (i, p) in px.iter_mut().enumerate() {
        let hash = (i as u32).wrapping_mul(2654435761);
        *p = 0.1 + (hash as f32 / u32::MAX as f32 - 0.5) * 0.01;
    }
    for k in 0..300 {
        let idx = (k as u32).wrapping_mul(2246822519) as usize % n;
        px[idx] = 0.95;
    }
    make_cfa(CR_W, CR_H, px, cfa)
}

#[quick_bench(warmup_iters = 1, iters = 3)]
fn bench_cosmic_ray_reject_mono(b: ::quickbench::Bencher) {
    let frame = cosmic_ray_frame(CfaType::Mono);
    let config = CosmicRayConfig::default();
    b.bench(|| {
        let mut f = frame.clone();
        black_box(reject_cosmic_rays(&mut f, black_box(&config)))
    });
}

#[quick_bench(warmup_iters = 1, iters = 3)]
fn bench_cosmic_ray_reject_bayer(b: ::quickbench::Bencher) {
    let frame = cosmic_ray_frame(bayer());
    let config = CosmicRayConfig::default();
    b.bench(|| {
        let mut f = frame.clone();
        black_box(reject_cosmic_rays(&mut f, black_box(&config)))
    });
}
