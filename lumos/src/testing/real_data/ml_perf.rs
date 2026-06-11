//! Whole-image ML timing probe (`#[ignore]`): loads the 125 MB model and runs star removal over the
//! *entire* bundled frame to report wall-clock cost. Tiles run sequentially with ORT's default
//! intra-op threading — concurrent Sessions were measured slower and memory-hungry on this
//! memory-bound model (see `ml/README.md`).
//!
//! Run: `cargo test -p lumos --release --features ml,real-data ml_full_image -- --ignored --nocapture`.

use std::time::Instant;

use super::ml_support::{onnx_weights, stretched_master};
use crate::ml::backend::TiledOnnxConfig;
use crate::ml::star_removal::remove_stars;
use crate::testing::init_tracing;

/// Count of 512-wide tiles covering `dim` at `stride` — mirrors the backend's `tile_starts`.
fn tiles_1d(dim: usize, stride: usize) -> usize {
    const WINDOW: usize = 512;
    let (mut n, mut x) = (1usize, 0usize);
    while x + WINDOW < dim {
        x += stride;
        n += 1;
    }
    n
}

#[test]
#[ignore = "perf probe: loads the 125MB model and processes the whole frame; run manually"]
fn ml_full_image_timing() {
    init_tracing();
    let Some(weights) = onnx_weights("STARNET2_ONNX", "StarNet2_weights.onnx") else {
        return;
    };
    let img = stretched_master();
    let (w, h, stride) = (img.width(), img.height(), 256usize);
    let tiles = tiles_1d(w, stride) * tiles_1d(h, stride);

    let t = Instant::now();
    remove_stars(&img, &TiledOnnxConfig::new(weights)).expect("star removal");
    let dt = t.elapsed().as_secs_f64();
    eprintln!(
        "FULL {w}x{h}: {tiles} tiles @ stride {stride} — {dt:.1}s ({:.0}ms/tile)",
        dt * 1000.0 / tiles as f64
    );
}
