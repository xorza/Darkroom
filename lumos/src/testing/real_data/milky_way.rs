//! Real-data "best Milky Way" pipeline: green-removal + stretch (as in the neutralize / SCNR /
//! renorm reference) with the new display-domain enhancers layered on — linear wavelet denoise,
//! HDR multiscale dynamic-range compression, and CLAHE local contrast — tuned for a wide-angle
//! Milky Way. Writes the stretched base and the enhanced result for side-by-side visual comparison.
//! Gated behind the `real-data` feature.

use crate::math::statistics::median_f32_mut;
use crate::testing::{calibration_dir, init_tracing, save_png};
use crate::{
    AstroImage, DenoiseConfig, HdrConfig, LocalContrastConfig, ScnrMethod, StretchConfig,
    compress_dynamic_range, denoise, enhance_local_contrast, neutralize_background, scnr, stretch,
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
    neutralize_background(&mut img); // equalize the green-elevated background
    denoise(&mut img, DenoiseConfig::default()); // gentle wavelet denoise (MW-tuned default)

    // --- Stretch + green removal, as in the neutralize / SCNR / renorm reference image. ---
    stretch(&mut img, StretchConfig::auto_stf());
    scnr(&mut img, ScnrMethod::AverageNeutral);
    neutralize_background(&mut img); // re-neutralize the now-display-domain background
    eprintln!("stretched base: median {:.3}", median(&img));
    assert_displayable(&img, "stretched base");
    save_png(&img, "milky_way/stretched.png");

    // --- Display-domain enhancement, tuned for a wide-angle Milky Way. ---
    // HDR: gently compress the bright star-cloud cores to reveal detail (small amount; too much
    // flattens the large-scale brightness).
    compress_dynamic_range(
        &mut img,
        HdrConfig {
            scales: 6,
            amount: 0.3,
        },
    );
    // Local contrast: pop the dust lanes / dark rifts (modest clip + strength so it doesn't crush
    // the background or over-sharpen the dense starfield).
    enhance_local_contrast(
        &mut img,
        LocalContrastConfig {
            tiles: 8,
            clip_limit: 2.0,
            strength: 0.6,
        },
    );
    scnr(&mut img, ScnrMethod::AverageNeutral); // final green touch-up after the enhancement

    eprintln!("enhanced: median {:.3}", median(&img));
    assert_displayable(&img, "enhanced");
    save_png(&img, "milky_way/enhanced.png");
}
