//! Real-data stretching: load the bundled stacked light frame, stretch it with each method, and
//! write viewable JPEGs for visual inspection. Gated behind the `real-data` feature (the dataset
//! lives in `test_data/lumos_data/`).

use crate::image_ops::intensity_plane;
use crate::io::image::linear::LinearImage;
use crate::math::statistics::median_f32_mut;
use crate::testing::{calibration_dir, init_tracing, save_png};
use crate::{ColorMode, NeutralizeBackground, Scnr, Stretch, StretchMethod};
use imaginarium::Image;

#[derive(Debug)]
struct Stats {
    min: f32,
    max: f32,
    median: f32,
}

fn stats(pixels: &[f32]) -> Stats {
    let min = pixels.iter().copied().fold(f32::INFINITY, f32::min);
    let max = pixels.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let mut scratch = pixels.to_vec();
    Stats {
        min,
        max,
        median: median_f32_mut(&mut scratch),
    }
}

#[test]
#[ignore = "real-data image-processing test; run explicitly with --ignored"]
fn stretch_stacked_light() {
    init_tracing();

    let path = calibration_dir().join("stacked_light.tiff");
    let mut image = Image::from(&LinearImage::from_file(&path).expect("load stacked_light.tiff"));
    let desc = image.desc;
    assert!(desc.width > 0 && desc.height > 0);

    NeutralizeBackground.apply(&mut image).unwrap();

    // A linear stacked deep-sky frame, before any display stretch: the calibrated background sits at
    // zero (a near-zero median — symmetric read noise dips some background pixels slightly negative,
    // which is correct calibration, not a defect) with a bright stellar tail whose peaks exceed 1.
    // The stretch caps the display output back into [0,1].
    let input = stats(intensity_plane(&image).pixels());
    eprintln!("input {}x{}: {input:?}", desc.width, desc.height);
    assert!(
        input.median.abs() < 0.05,
        "a calibrated linear background sits at zero: {input:?}"
    );
    assert!(
        input.max > 0.5,
        "has a bright stellar tail far above the zero background: {input:?}"
    );

    for (name, config) in [
        ("stf", Stretch::auto_stf()),
        ("asinh", Stretch::auto_asinh()),
        // GHS applied cold to linear data: the background sits at ~0, so the symmetry point is at 0
        // and the strength D must be large (the b=-1 logarithmic family lifts the faint signal).
        ("ghs", Stretch::ghs(5000.0, -1.0, 0.0)),
    ] {
        let mut stretched = image.clone();
        config.apply(&mut stretched).unwrap();

        let out = stats(intensity_plane(&stretched).pixels());
        eprintln!("{name}: {out:?}");

        assert!(
            out.min >= 0.0 && out.max <= 1.0 + 1e-3,
            "{name} output stays in [0,1]: {out:?}"
        );
        assert!(
            out.median > input.median,
            "{name} lifts the background out of the shadows: {} -> {}",
            input.median,
            out.median
        );
        assert!(
            out.max - out.min > 0.5,
            "{name} spreads contrast across the range: {out:?}"
        );

        Scnr::average_neutral().apply(&mut stretched).unwrap();
        save_png(&stretched, &format!("stretch/stacked_light_{name}.png"));
    }

    // Two-stage Milky-Way contrast — the realistic GHS workflow. A gentle auto-asinh first lifts the
    // dust into the mid-shadows (background median ~0.2), where GHS parameters are intuitive: a
    // contrast pass centered just above the background (`sp` on the dust), with `lp` keeping the
    // background dark and `hp` protecting the star cores. Far easier to focus on the Milky Way than
    // tuning GHS cold on linear data.
    let mut staged = image.clone();
    Stretch::auto_asinh().apply(&mut staged).unwrap();
    Stretch {
        method: StretchMethod::Ghs {
            d: 3.0,
            b: 0.0,
            sp: 0.3,
            lp: 0.15,
            hp: 0.9,
        },
        color: ColorMode::ColorPreserving,
    }
    .apply(&mut staged)
    .unwrap();
    let out = stats(intensity_plane(&staged).pixels());
    eprintln!("asinh+ghs: {out:?}");
    assert!(
        out.min >= 0.0 && out.max <= 1.0 + 1e-3,
        "asinh+ghs output stays in [0,1]: {out:?}"
    );
    Scnr::average_neutral().apply(&mut staged).unwrap();
    save_png(&staged, "stretch/stacked_light_asinh_ghs.png");
}
