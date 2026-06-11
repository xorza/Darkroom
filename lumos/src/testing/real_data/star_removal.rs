use super::ml_support::{center_crop, onnx_weights, stretched_master};
use crate::ml::backend::TiledOnnxConfig;
use crate::ml::star_removal::remove_stars;
use crate::testing::{init_tracing, save_png};

fn max_of(p: &[f32]) -> f32 {
    p.iter().copied().fold(0.0f32, f32::max)
}

/// Prototype: run a StarNet2 ONNX over a crop of the bundled (stretched) frame and write
/// input / starless / stars PNGs. Uses the gitignored, caller-supplied `StarNet2_weights.onnx` in
/// `test_data/` (lumos ships no model); `STARNET2_ONNX` overrides the path. Skipped if absent.
/// Build/run with `--features ml,real-data`.
#[test]
#[cfg_attr(not(feature = "real-data"), ignore)]
fn starnet_removes_stars_on_a_crop() {
    init_tracing();
    let Some(weights) = onnx_weights("STARNET2_ONNX", "StarNet2_weights.onnx") else {
        return;
    };

    // StarNet wants stretched display data in [0,1]; a 1024² centre crop is 9 tiles at stride 256.
    let img = stretched_master();
    let crop = center_crop(&img, 1024, 1024);
    save_png(&crop, "star_removal/input.png");

    let result =
        remove_stars(&crop, &TiledOnnxConfig::new(weights)).expect("star removal succeeds");
    save_png(&result.starless, "star_removal/starless.png");
    save_png(&result.stars, "star_removal/stars.png");

    // The starless image is no brighter than the input, and a non-trivial amount of (positive) star
    // signal was removed.
    let input = crop.intensity_plane();
    let starless = result.starless.intensity_plane();
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
}
