//! Benchmarks for local maxima deblending.
//!
//! Run with: `cargo test -p lumos --release --features bench bench_local_maxima -- --ignored
//! --nocapture`

use crate::testing::prelude::*;
use ::quickbench::quick_bench;
use std::cmp::Reverse;
use std::hint::black_box;

use crate::stacking::star_detection::deblend::component::Component;
use crate::stacking::star_detection::deblend::deblend_buffers::DeblendBuffers;
use crate::stacking::star_detection::deblend::internals::label_above;
use crate::stacking::star_detection::deblend::local_maxima::{
    Kept, deblend_local_maxima, find_local_maxima,
};
use crate::testing::synthetic::fixtures::cluster_field;

#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_find_local_maxima_6k_dense(b: ::quickbench::Bencher) {
    let pixels = cluster_field(Size2us::new(6144, 6144), 50000, 42)
        .image
        .channel(0)
        .clone();
    let labels = label_above(&pixels, 0.05);
    let components = labels.components();

    // Find the 100 largest components for benchmarking
    let mut sorted_components = components.to_vec();
    sorted_components.sort_by_key(|c| Reverse(c.area));
    let large_components: Vec<_> = sorted_components.into_iter().take(100).collect();

    let (mut maxima, mut peaks, mut occupied) = (Vec::new(), Vec::new(), Vec::new());
    b.bench(|| {
        for component in &large_components {
            find_local_maxima(
                &Component::new(black_box(component), &pixels, &labels),
                black_box(3),
                black_box(0.3),
                &mut maxima,
                Kept {
                    peaks: &mut peaks,
                    occupied: &mut occupied,
                },
            );
            black_box(&peaks);
        }
    });
}

#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_deblend_local_maxima_6k_dense(b: ::quickbench::Bencher) {
    let pixels = cluster_field(Size2us::new(6144, 6144), 50000, 42)
        .image
        .channel(0)
        .clone();
    let labels = label_above(&pixels, 0.05);
    let components = labels.components();

    let (mut buffers, mut regions) = (DeblendBuffers::default(), Vec::new());
    b.bench(|| {
        for component in components {
            regions.clear();
            black_box(deblend_local_maxima(
                &Component::new(black_box(component), &pixels, &labels),
                black_box(3),
                black_box(0.3),
                &mut buffers,
                &mut regions,
            ));
        }
    });
}

#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_local_maxima_4k_dense(b: ::quickbench::Bencher) {
    let pixels = cluster_field(Size2us::new(4096, 4096), 20000, 42)
        .image
        .channel(0)
        .clone();
    let labels = label_above(&pixels, 0.05);
    let components = labels.components();

    let (mut buffers, mut regions) = (DeblendBuffers::default(), Vec::new());
    b.bench(|| {
        for component in components {
            regions.clear();
            black_box(deblend_local_maxima(
                &Component::new(black_box(component), &pixels, &labels),
                black_box(3),
                black_box(0.3),
                &mut buffers,
                &mut regions,
            ));
        }
    });
}
