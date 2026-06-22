//! Real-data "best Milky Way" pipeline: green-removal + stretch (as in the neutralize / SCNR /
//! renorm reference) with the new display-domain enhancers layered on — linear wavelet denoise,
//! HDR multiscale dynamic-range compression, and CLAHE local contrast — tuned for a wide-angle
//! Milky Way. Writes the stretched base and the enhanced result for side-by-side visual comparison.
//! Gated behind the `real-data` feature.

use crate::color_calibration::{neutralize_background_planar, scnr_planar};
use crate::denoise::denoise_planar;
use crate::hdr::compress_dynamic_range_planar;
use crate::local_contrast::enhance_local_contrast_planar;
use crate::math::statistics::median_f32_mut;
use crate::stretching::stretch_planar;
use crate::testing::{calibration_dir, init_tracing, save_png};
use crate::{
    AstroImage, ColorMode, DenoiseConfig, HdrConfig, LocalContrastConfig, ScnrMethod,
    StretchConfig, StretchMethod,
};

fn median(image: &AstroImage) -> f32 {
    median_f32_mut(&mut image.intensity_plane().into_vec())
}

fn assert_displayable(image: &AstroImage, label: &str) {
    let plane = image.intensity_plane();
    let (min, max) = plane
        .pixels()
        .iter()
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), &v| {
            (lo.min(v), hi.max(v))
        });
    // The renorm `neutralize_background` shifts the background additively (so it dips negative — as
    // in the reference renorm image; save_png clamps the floor). Only the highlight ceiling is a hard
    // display limit; the floor just shouldn't be absurd.
    assert!(
        max <= 1.0 + 1e-3 && min > -0.5,
        "{label} is displayable: min {min} max {max}"
    );
}

#[test]
#[cfg_attr(not(feature = "real-data"), ignore)]
fn milky_way_best_pipeline() {
    init_tracing();
    let path = calibration_dir().join("stacked_light.tiff");
    let mut img = AstroImage::from_file(&path).expect("load stacked_light.tiff");

    // --- Linear domain (before the stretch): colour-calibrate, then denoise. ---
    neutralize_background_planar(&mut img); // equalize the green-elevated background
    denoise_planar(&mut img, DenoiseConfig::default()); // gentle wavelet denoise (MW-tuned default)

    // --- Stretch + green removal, as in the neutralize / SCNR / renorm reference image. ---
    stretch_planar(
        &mut img,
        StretchConfig {
            method: StretchMethod::AutoStf {
                shadow_sigmas: 1.5,
                target_background: 0.2,
            },
            color: ColorMode::ColorPreserving,
        },
    );
    // stretch_planar(&mut img, StretchConfig::auto_asinh());
    // stretch_planar(
    //     &mut img,
    //     StretchConfig {
    //         method: StretchMethod::Ghs {
    //             d: 3.0,
    //             b: 0.0,
    //             sp: 0.3,
    //             lp: 0.15,
    //             hp: 0.9,
    //         },
    //         color: ColorMode::ColorPreserving,
    //     },
    // );
    scnr_planar(&mut img, ScnrMethod::AverageNeutral);
    neutralize_background_planar(&mut img); // re-neutralize the now-display-domain background
    eprintln!("stretched base: median {:.3}", median(&img));
    assert_displayable(&img, "stretched base");
    save_png(&img, "milky_way/stretched.png");

    // --- Display-domain enhancement, tuned for a wide-angle Milky Way. ---
    // HDR: gently compress the bright star-cloud cores to reveal detail (small amount; too much
    // flattens the large-scale brightness).
    compress_dynamic_range_planar(
        &mut img,
        HdrConfig {
            scales: 6,
            amount: 0.3,
        },
    );
    // Local contrast: pop the dust lanes / dark rifts (modest clip + strength so it doesn't crush
    // the background or over-sharpen the dense starfield).
    enhance_local_contrast_planar(
        &mut img,
        LocalContrastConfig {
            tiles: 8,
            clip_limit: 2.0,
            strength: 0.6,
        },
    );
    scnr_planar(&mut img, ScnrMethod::AverageNeutral); // final green touch-up after the enhancement

    eprintln!("enhanced: median {:.3}", median(&img));
    assert_displayable(&img, "enhanced");
    save_png(&img, "milky_way/enhanced.png");
}
