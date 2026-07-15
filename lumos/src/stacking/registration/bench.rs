//! Benchmarks for the registration *solve* — triangle matching → RANSAC/MAGSAC → optional
//! SIP — on synthetic star fields. The image warp it feeds is benched separately in
//! `interpolation::bench`; real-data register/warp timing lives in `registration::real_data_tests`.
//!
//! Run: `cargo test -p lumos --release registration::bench -- --ignored --nocapture`

use glam::DVec2;
use quickbench::quick_bench;
use std::hint::black_box;

use crate::stacking::registration::transform::Transform;
use crate::testing::synthetic::fixtures::star_field;
use crate::{RegistrationConfig, Star, StarDetectionConfig, StarDetector, register};

/// Detect a realistic star set on a synthetic field, then build a registration target by
/// applying a known similarity transform to those stars — a clean, deterministic correspondence
/// set that still drives the full matching + RANSAC machinery over `num_stars` points.
fn star_pair(num_stars: usize, seed: u64) -> (Vec<Star>, Vec<Star>) {
    let frame = star_field(1500, 1500, num_stars, seed);
    let mut detector = StarDetector::from_config(StarDetectionConfig::default()).unwrap();
    let ref_stars = detector.detect(&frame.image).stars;
    // A modest rotation + scale + shift, the kind dithered subs differ by.
    let t = Transform::similarity(DVec2::new(11.0, -7.0), 0.03, 1.002);
    let target = ref_stars
        .iter()
        .map(|s| {
            let mut moved = *s;
            moved.pos = t.apply(s.pos);
            moved
        })
        .collect();
    (ref_stars, target)
}

#[quick_bench(warmup_iters = 3, iters = 20)]
fn bench_register_150_stars(b: ::quickbench::Bencher) {
    let (ref_stars, target) = star_pair(150, 7);
    let config = RegistrationConfig::default();
    b.bench(|| black_box(register(black_box(&ref_stars), black_box(&target), &config)));
}

#[quick_bench(warmup_iters = 3, iters = 10)]
fn bench_register_500_stars(b: ::quickbench::Bencher) {
    let (ref_stars, target) = star_pair(500, 9);
    let config = RegistrationConfig::default();
    b.bench(|| black_box(register(black_box(&ref_stars), black_box(&target), &config)));
}
