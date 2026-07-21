//! Whole-image ML timing probe (`#[ignore]`): loads the 125 MB model and runs star removal over the
//! *entire* bundled frame to report wall-clock cost. Tiles run sequentially with ORT's default
//! intra-op threading — concurrent Sessions were measured slower and memory-hungry on this
//! memory-bound model (see `ml/README.md`).
//!
//! Run: `cargo test -p lumos --release --features ml,real-data ml_full_image -- --ignored --nocapture`.

use std::time::Instant;

use crate::image_ops::ml::backend::TiledOnnxConfig;
use crate::image_ops::ml::star_removal::remove_stars;
use crate::testing::init_tracing;
use crate::testing::real_data::ml_support::{onnx_weights, stretched_master};

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
    let (w, h, stride) = (img.desc().width, img.desc().height, 256usize);
    let tiles = tiles_1d(w, stride) * tiles_1d(h, stride);

    let t = Instant::now();
    remove_stars(img, &TiledOnnxConfig::new(weights)).expect("star removal");
    let dt = t.elapsed().as_secs_f64();
    eprintln!(
        "FULL {w}x{h}: {tiles} tiles @ stride {stride} — {dt:.1}s ({:.0}ms/tile)",
        dt * 1000.0 / tiles as f64
    );
}

#[test]
#[ignore = "perf probe: isolates ONNX session/model load time from tile inference; run manually"]
fn ml_model_load_timing() {
    use ort::session::Session;

    init_tracing();
    let Some(weights) = onnx_weights("STARNET2_ONNX", "StarNet2_weights.onnx") else {
        return;
    };

    // Cold: first load in this process.
    let t = Instant::now();
    let _session = Session::builder()
        .expect("session builder")
        .commit_from_file(&weights)
        .expect("commit_from_file (model load)");
    let cold = t.elapsed().as_secs_f64();

    // Warm: file bytes are now in the OS page cache; a fresh Session still re-parses/re-plans
    // the graph from scratch (no cross-call session caching in `run_tiled`).
    let t = Instant::now();
    let _session2 = Session::builder()
        .expect("session builder")
        .commit_from_file(&weights)
        .expect("commit_from_file (model load)");
    let warm = t.elapsed().as_secs_f64();

    eprintln!(
        "model load {}: cold {cold:.3}s, warm (same process, page-cache hit) {warm:.3}s",
        weights.display()
    );

    let Some(denoise_weights) = onnx_weights("DEEPSNR_ONNX", "DeepSNR_weights_v2.onnx") else {
        return;
    };
    let t = Instant::now();
    let _session3 = Session::builder()
        .expect("session builder")
        .commit_from_file(&denoise_weights)
        .expect("commit_from_file (model load)");
    let deepsnr = t.elapsed().as_secs_f64();
    eprintln!("model load {}: {deepsnr:.3}s", denoise_weights.display());
}
