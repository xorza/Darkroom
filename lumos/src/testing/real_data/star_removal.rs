use std::path::PathBuf;

use crate::io::astro_image::{AstroImage, ImageDimensions};
use crate::ml::star_removal::{StarRemovalConfig, remove_stars};
use crate::testing::{calibration_dir, init_tracing, save_png};
use crate::{StretchConfig, neutralize_background, stretch};
use common::Vec2us;

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
    let weights = std::env::var_os("STARNET2_ONNX")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test_data/StarNet2_weights.onnx")
        });
    if !weights.exists() {
        eprintln!(
            "StarNet2 weights not found at {} (set STARNET2_ONNX or drop the .onnx there); skipping",
            weights.display()
        );
        return;
    }

    // StarNet wants stretched display data in [0,1]: neutralize + stretch the linear master first.
    let mut img =
        AstroImage::from_file(calibration_dir().join("stacked_light.tiff")).expect("load");
    neutralize_background(&mut img);
    stretch(&mut img, StretchConfig::auto_stf());

    // A 1024×1024 centre crop (9 tiles at stride 256) — quick to run as a prototype.
    let (cw, ch) = (1024, 1024);
    let crop = crop(
        &img,
        (img.width() - cw) / 2,
        (img.height() - ch) / 2,
        cw,
        ch,
    );
    save_png(&crop, "star_removal/input.png");

    let result =
        remove_stars(&crop, &StarRemovalConfig::new(weights)).expect("star removal succeeds");
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
