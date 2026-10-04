//! Real-data color calibration: load the bundled stacked light frame, neutralize its green
//! background in the linear domain, stretch, and SCNR — writing viewable JPEGs at each step so the
//! green cast can be seen disappearing. Gated behind the `real-data` feature.

use crate::image_ops::rgb::Rgb;
use crate::internals::visual;

use crate::image_ops::color_calibration::channel_backgrounds;
use crate::internals::init_tracing;
use crate::internals::real_data;
use crate::{NeutralizeBackground, Scnr, Stretch};

fn spread(bg: Rgb) -> f32 {
    bg.r.max(bg.g).max(bg.b) - bg.r.min(bg.g).min(bg.b)
}

#[test]
fn neutralize_then_stretch_removes_green() {
    init_tracing();

    let image = real_data::linear_master();

    // The raw OSC stack has a colored (green-elevated) background: the per-channel backgrounds
    // differ.
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
    NeutralizeBackground.apply(&mut img).unwrap();
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
    // `stretching::tests::real_data`).
    Stretch::auto_stf().apply(&mut img).unwrap();
    visual::save_linear(&img, "color/stacked_light_neutralized_stf");

    // Post-stretch Average-Neutral SCNR cleans any residual green left after neutralization.
    Scnr::average_neutral(1.0).apply(&mut img).unwrap();
    visual::save_linear(&img, "color/stacked_light_neutralized_scnr");

    NeutralizeBackground.apply(&mut img).unwrap();
    visual::save_linear(&img, "color/stacked_light_neutralized_scnr_renorm");
}
