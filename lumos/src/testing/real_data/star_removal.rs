use super::ml_support::{onnx_weights, stretched_master};
use crate::image_ops::intensity_plane;
use crate::image_ops::ml::backend::TiledOnnxConfig;
use crate::image_ops::ml::star_removal::{remove_stars, remove_stars_starless_only};
use crate::testing::{init_tracing, save_png};

fn max_of(p: &[f32]) -> f32 {
    p.iter().copied().fold(0.0f32, f32::max)
}

/// Prototype: run a StarNet2 ONNX over the full bundled (stretched) frame and write
/// input / starless / stars PNGs. Uses the gitignored, caller-supplied `StarNet2_weights.onnx` in
/// `test_data/` (lumos ships no model); `STARNET2_ONNX` overrides the path. Skipped if absent. The
/// full frame is hundreds of 512² tiles — ~60 s on a 10-core machine. Build/run with
/// `--features ml,real-data`.
#[test]
#[cfg_attr(not(feature = "real-data"), ignore)]
fn starnet_removes_stars() {
    init_tracing();
    let Some(weights) = onnx_weights("STARNET2_ONNX", "StarNet2_weights.onnx") else {
        return;
    };

    // StarNet wants stretched display data in [0,1].
    let img = stretched_master();
    save_png(&img, "star_removal/input.png");

    let config = TiledOnnxConfig::new(weights);
    let result = remove_stars(&img, &config).expect("star removal succeeds");
    save_png(&result.starless, "star_removal/starless.png");
    save_png(&result.stars, "star_removal/stars.png");

    // The starless image is no brighter than the input, and a non-trivial amount of (positive) star
    // signal was removed.
    let input = intensity_plane(&img);
    let starless = intensity_plane(&result.starless);
    let in_max = max_of(input.pixels());
    let sl_max = max_of(starless.pixels());
    let removed: f32 = input
        .pixels()
        .iter()
        .zip(starless.pixels())
        .map(|(&a, &b)| (a - b).max(0.0))
        .sum::<f32>()
        / input.len() as f32;
    eprintln!("in_max {in_max:.3}  starless_max {sl_max:.3}  mean removed {removed:.4}");
    assert!(sl_max <= in_max + 1e-3, "starless not brighter than input");
    assert!(removed > 1e-4, "star signal was removed (mean {removed})");

    // The starless-only path (used when a caller doesn't need the `stars`
    // layer) must reproduce `remove_stars`'s `starless` output exactly —
    // it's the same underlying `run_tiled` inference either way.
    let starless_only =
        remove_stars_starless_only(&img, &config).expect("starless-only star removal succeeds");
    assert_eq!(starless_only.bytes(), result.starless.bytes());
}
