//! Real-data background (gradient) extraction on the bundled stacked master. Background extraction
//! is *canonically* a linear-domain step (see `background_extraction/README.md`); this exercises it
//! on the **stretched** master to flatten the display-domain background, saving a viewable
//! before/after. Gated behind the `real-data` feature.

use crate::background_extraction::extract_background_planar;
use crate::color_calibration::{neutralize_background_planar, scnr_planar};
use crate::math::statistics::median_f32_mut;
use crate::stretching::stretch_planar;
use crate::testing::{calibration_dir, init_tracing, save_png};
use crate::{AstroImage, BackgroundConfig, ScnrMethod, StretchConfig};

/// Max−min of the robust background level across the four corners of the intensity plane — a proxy
/// for the corner-to-corner gradient. A light-pollution gradient makes opposite corners differ;
/// flattening the background drives them together.
fn corner_background_spread(image: &AstroImage) -> f32 {
    let plane = image.intensity_plane();
    let (w, h) = (plane.width(), plane.height());
    let px = plane.pixels();
    let patch = 256.min(w / 4).min(h / 4);
    let inset = patch / 2;

    let patch_median = |x0: usize, y0: usize| -> f32 {
        let mut vals: Vec<f32> = Vec::with_capacity(patch * patch);
        for y in y0..y0 + patch {
            let row = y * w + x0;
            vals.extend_from_slice(&px[row..row + patch]);
        }
        median_f32_mut(&mut vals)
    };

    let corners = [
        patch_median(inset, inset),
        patch_median(w - inset - patch, inset),
        patch_median(inset, h - inset - patch),
        patch_median(w - inset - patch, h - inset - patch),
    ];
    let max = corners.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let min = corners.iter().copied().fold(f32::INFINITY, f32::min);
    max - min
}

#[test]
#[cfg_attr(not(feature = "real-data"), ignore)]
fn extract_flattens_background_on_stretched_master() {
    init_tracing();

    // The display-domain master, as the other real-data tests build it.
    let mut img =
        AstroImage::from_file(calibration_dir().join("stacked_light.tiff")).expect("load");
    neutralize_background_planar(&mut img);
    stretch_planar(&mut img, StretchConfig::auto_stf());
    scnr_planar(&mut img, ScnrMethod::AverageNeutral);
    save_png(&img, "bg_extraction/stretched.png");

    let before = corner_background_spread(&img);

    // Model and subtract the smooth background per channel, then re-neutralize so the now-flattened
    // background sits at a viewable level (subtraction pulls it toward zero).
    let mut extracted = img.clone();
    extract_background_planar(&mut extracted, &BackgroundConfig::default());
    neutralize_background_planar(&mut extracted);
    save_png(&extracted, "bg_extraction/extracted.png");

    let after = corner_background_spread(&extracted);
    eprintln!(
        "corner background spread: {before:.5} -> {after:.5}  ({:.0}% reduction)",
        100.0 * (1.0 - after / before)
    );

    // The bundled master carries a strong light-pollution gradient (corners differ by ~0.23 of the
    // display range); extraction removes most of it. Observed ≈ 0.226 -> 0.036 (84% reduction).
    assert!(
        before > 0.1,
        "precondition: the stretched master has a real corner gradient, got spread {before:.5}"
    );
    assert!(
        after < 0.4 * before,
        "background extraction removes most of the corner gradient: {before:.5} -> {after:.5}"
    );
}
