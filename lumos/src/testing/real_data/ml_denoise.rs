use crate::image_ops::intensity_plane;
use crate::image_ops::ml::backend::TiledOnnxConfig;
use crate::image_ops::ml::denoise::ml_denoise;
use crate::testing::real_data::ml_support::{onnx_weights, stretched_master};
use crate::testing::{init_tracing, save_png};
use imaginarium::Image;

/// Mean |adjacent-pixel difference| of the intensity — a high-frequency noise proxy (slow gradients
/// cancel; pixel-scale grain is what a denoiser removes).
fn mean_adjacent_diff(image: &Image) -> f32 {
    let plane = intensity_plane(image);
    let w = plane.width();
    let px = plane.pixels();
    let (mut sum, mut n) = (0.0f32, 0u64);
    for (i, &v) in px.iter().enumerate() {
        if i % w != w - 1 {
            sum += (px[i + 1] - v).abs();
            n += 1;
        }
    }
    sum / n as f32
}

/// Prototype: run a DeepSNR-style ONNX denoiser over the full bundled (stretched) frame and
/// write input / denoised PNGs. Uses the gitignored `DeepSNR_weights_v2.onnx` in `test_data/`
/// (lumos ships no model); `DEEPSNR_ONNX` overrides the path. Skipped if absent. The full frame is
/// hundreds of 512² tiles — ~60 s on a 10-core machine. Build/run with `--features ml,real-data`.
#[test]
#[ignore = "real-data ML test loads a large model; run explicitly with --ignored"]
fn deepsnr_denoises() {
    init_tracing();
    let Some(weights) = onnx_weights("DEEPSNR_ONNX", "DeepSNR_weights_v2.onnx") else {
        return;
    };

    // CNN denoisers want stretched display data in [0,1].
    let img = stretched_master();
    save_png(&img, "ml_denoise/input.png");

    let denoised = ml_denoise(&img, &TiledOnnxConfig::new(weights)).expect("denoise succeeds");
    save_png(&denoised, "ml_denoise/denoised.png");

    let in_hf = mean_adjacent_diff(&img);
    let out_hf = mean_adjacent_diff(&denoised);
    eprintln!("high-frequency noise: {in_hf:.5} -> {out_hf:.5}");
    assert!(
        out_hf < in_hf,
        "denoised has less high-frequency noise: {out_hf} < {in_hf}"
    );
}
