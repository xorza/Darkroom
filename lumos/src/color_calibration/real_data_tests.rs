//! Real-data color calibration: load the bundled stacked light frame, neutralize its green
//! background in the linear domain, stretch, and SCNR — writing viewable JPEGs at each step so the
//! green cast can be seen disappearing. Gated behind the `real-data` feature.

use common::test_utils::test_output_path;
use imaginarium::{ColorFormat, Image};

use super::{ScnrMethod, channel_backgrounds, neutralize_background, scnr};
use crate::testing::{calibration_dir, init_tracing};
use crate::{AstroImage, StretchConfig, stretch};
use common::Rgb;

fn save_jpg(image: &AstroImage, name: &str) {
    let path = test_output_path(name);
    std::fs::create_dir_all(path.parent().unwrap()).expect("create test_output dir");
    Image::from(image)
        .convert(ColorFormat::RGB_U8)
        .expect("convert to RGB_U8")
        .save_file(&path)
        .expect("save jpg");
    eprintln!("wrote {}", path.display());
}

fn spread(bg: Rgb) -> f32 {
    bg.r.max(bg.g).max(bg.b) - bg.r.min(bg.g).min(bg.b)
}

#[test]
#[cfg_attr(not(feature = "real-data"), ignore)]
fn neutralize_then_stretch_removes_green() {
    init_tracing();

    let image = AstroImage::from_file(calibration_dir().join("stacked_light.tiff")).expect("load");

    // The raw OSC stack has a colored (green-elevated) background: the per-channel backgrounds differ.
    let before = channel_backgrounds(&image);
    let spread_before = spread(before);
    eprintln!(
        "background before: R={} G={} B={}  (spread {spread_before:.6})",
        before.r, before.g, before.b
    );
    assert!(
        spread_before > 1e-5,
        "the raw stack has a colored background: {before:?}"
    );

    // Neutralize in the linear domain → background goes neutral (all channels to a common level).
    let mut img = image.clone();
    neutralize_background(&mut img);
    let after = channel_backgrounds(&img);
    let spread_after = spread(after);
    eprintln!(
        "background after:  R={} G={} B={}  (spread {spread_after:.6})",
        after.r, after.g, after.b
    );
    assert!(
        spread_after < spread_before,
        "neutralization reduced the color spread"
    );
    assert!(
        spread_after < 1e-5,
        "backgrounds neutralized to a common level: {after:?}"
    );

    // Neutralized → stretch → save (compare against the un-neutralized green stretch from
    // `stretching::real_data_tests`).
    stretch(&mut img, StretchConfig::auto_stf());
    save_jpg(&img, "color/stacked_light_neutralized_stf.jpg");

    // Post-stretch Average-Neutral SCNR cleans any residual green left after neutralization.
    scnr(&mut img, ScnrMethod::AverageNeutral);
    save_jpg(&img, "color/stacked_light_neutralized_scnr.jpg");
}
