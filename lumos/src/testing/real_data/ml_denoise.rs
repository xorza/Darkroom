use super::ml_support::{center_crop, onnx_weights, stretched_master};
use crate::AstroImage;
use crate::ml::backend::TiledOnnxConfig;
use crate::ml::denoise::ml_denoise;
use crate::testing::{init_tracing, save_png};

/// Mean |adjacent-pixel difference| of the intensity — a high-frequency noise proxy (slow gradients
/// cancel; pixel-scale grain is what a denoiser removes).
fn mean_adjacent_diff(image: &AstroImage) -> f32 {
    let plane = image.intensity_plane();
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

/// Prototype: run a DeepSNR-style ONNX denoiser over a crop of the bundled (stretched) frame and
/// write input / denoised PNGs. Uses the gitignored `DeepSNR_weights_v2.onnx` in `test_data/`
/// (lumos ships no model); `DEEPSNR_ONNX` overrides the path. Skipped if absent. Build/run with
/// `--features ml,real-data`.
#[test]
#[cfg_attr(not(feature = "real-data"), ignore)]
fn deepsnr_denoises_a_crop() {
    init_tracing();
    let Some(weights) = onnx_weights("DEEPSNR_ONNX", "DeepSNR_weights_v2.onnx") else {
        return;
    };

    // CNN denoisers want stretched display data in [0,1].
    let img = stretched_master();
    let crop = center_crop(&img, 1024, 1024);
    save_png(&crop, "ml_denoise/input.png");

    let denoised = ml_denoise(&crop, &TiledOnnxConfig::new(weights)).expect("denoise succeeds");
    save_png(&denoised, "ml_denoise/denoised.png");

    let in_hf = mean_adjacent_diff(&crop);
    let out_hf = mean_adjacent_diff(&denoised);
    eprintln!("high-frequency noise: {in_hf:.5} -> {out_hf:.5}");
    assert!(
        out_hf < in_hf,
        "denoised has less high-frequency noise: {out_hf} < {in_hf}"
    );
}
