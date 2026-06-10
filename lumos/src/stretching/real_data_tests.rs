//! Real-data stretching: load the bundled stacked light frame, stretch it with each method, and
//! write viewable JPEGs for visual inspection. Gated behind the `real-data` feature (the dataset
//! lives in `test_data/lumos_data/`).

use crate::math::statistics::median_f32_mut;
use crate::testing::{calibration_dir, init_tracing, save_png};
use crate::{AstroImage, StretchConfig, stretch};

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
#[cfg_attr(not(feature = "real-data"), ignore)]
fn stretch_stacked_light() {
    init_tracing();

    let path = calibration_dir().join("stacked_light.tiff");
    let image = AstroImage::from_file(&path).expect("load stacked_light.tiff");
    let (w, h, ch) = (image.width(), image.height(), image.channels());
    assert!(w > 0 && h > 0);

    // A linear stacked deep-sky frame, before any display stretch: the calibrated background sits at
    // zero (a near-zero median — symmetric read noise dips some background pixels slightly negative,
    // which is correct calibration, not a defect) with a bright stellar tail whose peaks exceed 1.
    // The stretch caps the display output back into [0,1].
    let input = stats(image.intensity_plane().pixels());
    eprintln!("input {w}x{h} {ch}ch: {input:?}");
    assert!(
        input.median.abs() < 0.05,
        "a calibrated linear background sits at zero: {input:?}"
    );
    assert!(
        input.max > 0.5,
        "has a bright stellar tail far above the zero background: {input:?}"
    );

    for (name, config) in [
        ("stf", StretchConfig::auto_stf()),
        ("asinh", StretchConfig::auto_asinh()),
    ] {
        let mut stretched = image.clone();
        stretch(&mut stretched, config);

        let out = stats(stretched.intensity_plane().pixels());
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

        save_png(&stretched, &format!("stretch/stacked_light_{name}.png"));
    }
}
