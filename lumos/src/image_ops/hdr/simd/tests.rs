//! Every tier against `Portable`, the model the kernels' `ln` and `exp` are specified on, over a
//! mono band and an RGB one, and the values the kernels give on hand-picked pixels.

use common::Libm;
use simd::{IsaTier, Kernel};

use crate::image::intensity::IntensityBand;
use crate::image_ops::hdr::LOG_FLOOR;
use crate::image_ops::hdr::simd::{CompressFactor, LogIntensity};
use crate::internals::simd_check::{SWEEP_WIDTHS, ScalarSimd, assert_simd_matches_scalar};

#[test]
fn log_intensity_matches_portable() {
    assert_simd_matches_scalar(SWEEP_WIDTHS, |tier, shape, width| {
        let channels = [0, 1, 2].map(|seed| shape.row(width, seed));
        let mut out = Vec::new();
        for band in [
            IntensityBand::Mono(&channels[0]),
            IntensityBand::Rgb([&channels[0], &channels[1], &channels[2]]),
        ] {
            let (mut model, mut tiered) = (vec![0.0; width], vec![0.0; width]);
            for (isa_tier, out) in [(IsaTier::portable(), &mut model), (tier, &mut tiered)] {
                isa_tier.run(LogIntensity {
                    intensity: band,
                    out,
                });
            }
            out.push((model, tiered));
        }
        let (model, tiered): (Vec<_>, Vec<_>) = out.into_iter().unzip();
        ScalarSimd::new(model.concat(), tiered.concat())
    });
}

#[test]
fn compress_factor_matches_portable() {
    assert_simd_matches_scalar(SWEEP_WIDTHS, |tier, shape, width| {
        let channels = [0, 1, 2].map(|seed| shape.row(width, seed));
        let residual = shape.row(width, 3);
        let mut out = Vec::new();
        for band in [
            IntensityBand::Mono(&channels[0]),
            IntensityBand::Rgb([&channels[0], &channels[1], &channels[2]]),
        ] {
            let (mut model, mut tiered) = (residual.clone(), residual.clone());
            for (isa_tier, residual) in [(IsaTier::portable(), &mut model), (tier, &mut tiered)] {
                isa_tier.run(CompressFactor {
                    intensity: band,
                    residual,
                    mean: 0.25,
                    amount: 0.5,
                });
            }
            out.push((model, tiered));
        }
        let (model, tiered): (Vec<_>, Vec<_>) = out.into_iter().unzip();
        ScalarSimd::new(model.concat(), tiered.concat())
    });
}

/// The log of the floor, `−16·ln 2`, stands for every intensity at or below it and for a NaN; an
/// infinite one takes `ln f32::MAX`, 88.72; 1 is exactly 0 and `e` 1, within ε. An RGB pixel is
/// read through its mean: `(0.5 + 1 + 1.5)/3` is 1, after a ninth lane that leaves a partial
/// vector.
#[test]
fn log_intensity_floors_and_reads_the_mean() {
    let mono = [
        f32::NAN,
        0.0,
        -1.0,
        LOG_FLOOR / 2.0,
        LOG_FLOOR,
        f32::INFINITY,
        1.0,
        std::f32::consts::E,
        1.0,
    ];
    let mut out = [0.0; 9];
    LogIntensity {
        intensity: IntensityBand::Mono(&mono),
        out: &mut out,
    }
    .dispatch();
    let floor = -16.0 * std::f64::consts::LN_2;
    let near =
        |got: f32, want: f64| (f64::from(got) - want).abs() <= f64::from(f32::EPSILON) * want.abs();
    for (i, &got) in out[..5].iter().enumerate() {
        assert!(near(got, floor), "[{i}] {got}");
    }
    assert!(near(out[5], f64::from(f32::MAX).libm_ln()), "{}", out[5]);
    assert_eq!(out[6], 0.0);
    assert!(near(out[7], 1.0), "{}", out[7]);
    assert_eq!(out[8], 0.0);

    let (red, green, blue) = ([0.5; 9], [1.0; 9], [1.5; 9]);
    let mut out = [f32::NAN; 9];
    LogIntensity {
        intensity: IntensityBand::Rgb([&red, &green, &blue]),
        out: &mut out,
    }
    .dispatch();
    assert_eq!(out, [0.0; 9]);
}

/// A residual at the mean leaves the intensity as it is, `exp(0) = 1` exactly; one `2 ln 2` above
/// it at amount ½ halves it, `exp(−ln 2)`, within ε.
#[test]
fn compress_factor_moves_the_residual_toward_the_mean() {
    let mean = 0.25;
    let intensity = [0.8, 0.8, 0.3];
    let mut residual = [mean, mean + 2.0 * std::f32::consts::LN_2, mean];
    CompressFactor {
        intensity: IntensityBand::Mono(&intensity),
        residual: &mut residual,
        mean,
        amount: 0.5,
    }
    .dispatch();
    assert_eq!([residual[0], residual[2]], [0.8, 0.3]);
    assert!(
        (residual[1] - 0.4).abs() <= f32::EPSILON * 0.4,
        "{}",
        residual[1]
    );
}
