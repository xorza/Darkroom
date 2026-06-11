use std::path::PathBuf;

use common::Vec2us;

use crate::io::astro_image::{AstroImage, ImageDimensions};
use crate::ml::backend::TiledOnnxConfig;
use crate::ml::denoise::ml_denoise;
use crate::testing::{calibration_dir, init_tracing, save_png};
use crate::{StretchConfig, neutralize_background, stretch};

/// Copy a `cw×ch` sub-region starting at `(x0, y0)` into a fresh image.
fn crop(image: &AstroImage, x0: usize, y0: usize, cw: usize, ch: usize) -> AstroImage {
    let iw = image.width();
    let channels: Vec<Vec<f32>> = (0..image.channels())
        .map(|c| {
            let src = image.channel(c).pixels();
            let mut out = Vec::with_capacity(cw * ch);
            for yy in 0..ch {
                let r = (y0 + yy) * iw + x0;
                out.extend_from_slice(&src[r..r + cw]);
            }
            out
        })
        .collect();
    AstroImage::from_planar_channels(
        ImageDimensions::new(Vec2us::new(cw, ch), image.channels()),
        channels,
    )
}

/// Mean |adjacent-pixel difference| of the intensity — a high-frequency noise proxy (slow gradients
/// cancel; pixel-scale grain is what a denoiser removes).
fn highfreq_noise(image: &AstroImage) -> f32 {
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
    let weights = std::env::var_os("DEEPSNR_ONNX")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test_data/DeepSNR_weights_v2.onnx")
        });
    if !weights.exists() {
        eprintln!(
            "DeepSNR weights not found at {} (set DEEPSNR_ONNX or drop the .onnx there); skipping",
            weights.display()
        );
        return;
    }

    // CNN denoisers want stretched display data in [0,1]: neutralize + stretch first.
    let mut img =
        AstroImage::from_file(calibration_dir().join("stacked_light.tiff")).expect("load");
    neutralize_background(&mut img);
    stretch(&mut img, StretchConfig::auto_stf());

    let (cw, ch) = (1024, 1024);
    let crop = crop(
        &img,
        (img.width() - cw) / 2,
        (img.height() - ch) / 2,
        cw,
        ch,
    );
    save_png(&crop, "ml_denoise/input.png");

    let denoised = ml_denoise(&crop, &TiledOnnxConfig::new(weights)).expect("denoise succeeds");
    save_png(&denoised, "ml_denoise/denoised.png");

    let in_hf = highfreq_noise(&crop);
    let out_hf = highfreq_noise(&denoised);
    eprintln!("high-frequency noise: {in_hf:.5} -> {out_hf:.5}");
    assert!(
        out_hf < in_hf,
        "denoised has less high-frequency noise: {out_hf} < {in_hf}"
    );
}
