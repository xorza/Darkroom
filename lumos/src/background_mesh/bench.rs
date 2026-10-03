//! Benchmarks for the tile mesh.

use ::quickbench::quick_bench;
use std::hint::black_box;

use crate::background_mesh::workspace::MeshWorkspace;
use crate::bit_buffer2::BitBuffer2;
use crate::internals::synthetic::fixtures::cluster_field;
use crate::math::size2us::Size2us;

const BENCH_SIGMA_CLIP_ITERATIONS: usize = 2;

#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_tile_grid_6k_globular(b: ::quickbench::Bencher) {
    let pixels = cluster_field(Size2us::new(6144, 6144), 50000, 42)
        .image
        .channel(0)
        .clone();
    let mut workspace = MeshWorkspace::default();

    b.bench(|| {
        let grid = workspace.compute(&pixels, None, 64, BENCH_SIGMA_CLIP_ITERATIONS, true);
        black_box(grid);
    });
}

#[quick_bench(warmup_time_ms = 200, bench_time_ms = 1000)]
fn bench_tile_grid_6k_with_mask(b: ::quickbench::Bencher) {
    let pixels = cluster_field(Size2us::new(6144, 6144), 50000, 42)
        .image
        .channel(0)
        .clone();

    // Create mask from actual bright pixels (threshold at 0.1)
    let width = pixels.width();
    let height = pixels.height();
    let mut mask = BitBuffer2::new_filled(Size2us::new(width, height), false);

    for (idx, &val) in pixels.iter().enumerate() {
        if val > 0.1 {
            mask.set(idx, true);
        }
    }

    let masked_count: usize = mask.words.iter().map(|w| w.count_ones() as usize).sum();
    println!(
        "Mask: {} pixels masked ({:.1}%)",
        masked_count,
        100.0 * masked_count as f64 / (width * height) as f64
    );

    let mut workspace = MeshWorkspace::default();

    b.bench(|| {
        let grid = workspace.compute(&pixels, Some(&mask), 64, BENCH_SIGMA_CLIP_ITERATIONS, true);
        black_box(grid);
    });
}
