//! Real-data "best Milky Way" pipeline: green-removal + stretch (as in the neutralize / SCNR /
//! renorm reference) with the new display-domain enhancers layered on — linear wavelet denoise,
//! HDR multiscale dynamic-range compression, and CLAHE local contrast — tuned for a wide-angle
//! Milky Way. Writes the stretched base and the enhanced result for side-by-side visual comparison.
//! Gated behind the `real-data` feature.

use crate::image_ops::intensity_plane;
use crate::io::image::linear::LinearImage;
use crate::math::statistics::median_f32_mut;
use crate::testing::{calibration_dir, init_tracing, save_png};
use crate::{
    ColorMode, Denoise, Hdr, LocalContrast, NeutralizeBackground, Scnr, Stretch, StretchMethod,
};
use imaginarium::Image;

fn median(image: &Image) -> f32 {
    median_f32_mut(&mut intensity_plane(image).into_vec())
}

fn assert_displayable(image: &Image, label: &str) {
    let plane = intensity_plane(image);
    let (min, max) = plane
        .pixels()
        .iter()
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), &v| {
            (lo.min(v), hi.max(v))
        });
    // The renorm `NeutralizeBackground` shifts the background additively (so it dips negative — as
    // in the reference renorm image; save_png clamps the floor). Only the highlight ceiling is a hard
    // display limit; the floor just shouldn't be absurd.
    assert!(
        max <= 1.0 + 1e-3 && min > -0.5,
        "{label} is displayable: min {min} max {max}"
    );
}

#[test]
#[ignore = "real-data image-processing test; run explicitly with --ignored"]
fn milky_way_best_pipeline() {
    init_tracing();
    let path = calibration_dir().join("stacked_light.tiff");
    let mut img = Image::from(&LinearImage::from_file(&path).expect("load stacked_light.tiff"));

    NeutralizeBackground.apply(&mut img).unwrap(); // equalize the green-elevated background
    Denoise::default().apply(&mut img).unwrap(); // gentle wavelet denoise (MW-tuned default)

    Stretch {
        method: StretchMethod::AutoStf {
            shadow_sigmas: 1.5,
            target_background: 0.2,
        },
        color: ColorMode::ColorPreserving,
    }
    .apply(&mut img)
    .unwrap();
    Scnr::average_neutral().apply(&mut img).unwrap();
    NeutralizeBackground.apply(&mut img).unwrap(); // re-neutralize the now-display-domain background
    eprintln!("stretched base: median {:.3}", median(&img));
    assert_displayable(&img, "stretched base");
    save_png(&img, "milky_way/stretched.png");

    // HDR: gently compress the bright star-cloud cores to reveal detail (small amount; too much
    // flattens the large-scale brightness).
    Hdr {
        scales: 6,
        amount: 0.3,
    }
    .apply(&mut img)
    .unwrap();
    // Local contrast: pop the dust lanes / dark rifts (modest clip + strength so it doesn't crush
    // the background or over-sharpen the dense starfield).
    LocalContrast {
        tiles: 8,
        clip_limit: 2.0,
        strength: 0.6,
    }
    .apply(&mut img)
    .unwrap();
    Scnr::average_neutral().apply(&mut img).unwrap(); // final green touch-up after the enhancement

    eprintln!("enhanced: median {:.3}", median(&img));
    assert_displayable(&img, "enhanced");
    save_png(&img, "milky_way/enhanced.png");
}
