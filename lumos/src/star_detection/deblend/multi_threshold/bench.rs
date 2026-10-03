//! Benchmarks for multi-threshold deblending.
//!
//! Run with: `cargo test -p lumos --release --features bench bench_multi_threshold -- --ignored
//! --nocapture`

use crate::internals::prelude::*;
use ::quickbench::quick_bench;
use std::hint::black_box;

use crate::internals::synthetic::fixtures::cluster_field;
use crate::star_detection::config::detection_config::Connectivity;
use crate::star_detection::deblend::component::Component;
use crate::star_detection::deblend::deblend_buffers::DeblendBuffers;
use crate::star_detection::deblend::internals::label_above;
use crate::star_detection::deblend::multi_threshold::{
    MultiThresholdParams, deblend_multi_threshold,
};

#[quick_bench(warmup_iters = 1, iters = 3)]
fn bench_deblend_multi_threshold_6k_dense(b: ::quickbench::Bencher) {
    let pixels = cluster_field(Size2us::new(6144, 6144), 50000, 42)
        .image
        .channel(0)
        .clone();
    let labels = label_above(&pixels, 0.05);
    let components = labels.components();

    // Filter out huge components - multi-threshold is O(n * n_thresholds) per component
    // and not practical for >100k pixels
    let reasonable_components: Vec<_> = components.iter().filter(|c| c.area < 100_000).collect();

    let n_thresholds = 32;
    let min_separation = 3;
    let min_contrast = 0.005;

    let (mut buffers, mut regions) = (DeblendBuffers::default(), Vec::new());

    b.bench(|| {
        for component in &reasonable_components {
            regions.clear();
            black_box(deblend_multi_threshold(
                &Component::new(black_box(component), &pixels, &labels),
                0.05,
                MultiThresholdParams {
                    n_thresholds,
                    min_contrast,
                    min_separation,
                    connectivity: Connectivity::Four,
                },
                &mut buffers,
                &mut regions,
            ));
        }
    });
}

#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_deblend_multi_threshold_6k_dense_fewer_levels(b: ::quickbench::Bencher) {
    let pixels = cluster_field(Size2us::new(6144, 6144), 50000, 42)
        .image
        .channel(0)
        .clone();
    let labels = label_above(&pixels, 0.05);
    let components = labels.components();

    let reasonable_components: Vec<_> = components
        .iter()
        .filter(|c| c.area < 100_000)
        .take(5000)
        .collect();

    let n_thresholds = 16;
    let min_separation = 3;
    let min_contrast = 0.005;

    let (mut buffers, mut regions) = (DeblendBuffers::default(), Vec::new());

    b.bench(|| {
        for component in &reasonable_components {
            regions.clear();
            black_box(deblend_multi_threshold(
                &Component::new(black_box(component), &pixels, &labels),
                0.05,
                MultiThresholdParams {
                    n_thresholds,
                    min_contrast,
                    min_separation,
                    connectivity: Connectivity::Four,
                },
                &mut buffers,
                &mut regions,
            ));
        }
    });
}

#[quick_bench(warmup_iters = 1, iters = 3)]
fn bench_multi_threshold_4k_dense(b: ::quickbench::Bencher) {
    let pixels = cluster_field(Size2us::new(4096, 4096), 20000, 42)
        .image
        .channel(0)
        .clone();
    let labels = label_above(&pixels, 0.05);
    let components = labels.components();

    let reasonable_components: Vec<_> = components.iter().filter(|c| c.area < 100_000).collect();

    let n_thresholds = 32;
    let min_separation = 3;
    let min_contrast = 0.005;

    // Reuse buffers across components (same as real pipeline via rayon fold)
    let (mut buffers, mut regions) = (DeblendBuffers::default(), Vec::new());

    b.bench(|| {
        for component in &reasonable_components {
            regions.clear();
            black_box(deblend_multi_threshold(
                &Component::new(black_box(component), &pixels, &labels),
                0.05,
                MultiThresholdParams {
                    n_thresholds,
                    min_contrast,
                    min_separation,
                    connectivity: Connectivity::Four,
                },
                &mut buffers,
                &mut regions,
            ));
        }
    });
}
