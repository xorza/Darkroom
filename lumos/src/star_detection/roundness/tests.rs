use crate::star_detection::roundness::{Roundness, RoundnessStamp};

/// A sampled Gaussian of `(σx, σy)` and rotation `angle` at `(dx, dy)` from the centre of a stamp
/// of `radius`.
fn stamp(radius: usize, sigma: (f64, f64), angle: f64, offset: (f64, f64)) -> Vec<f64> {
    let size = 2 * radius + 1;
    let (sin, cos) = angle.sin_cos();
    (0..size * size)
        .map(|index| {
            let x = (index % size) as f64 - radius as f64 - offset.0;
            let y = (index / size) as f64 - radius as f64 - offset.1;
            let (u, v) = (x * cos + y * sin, -x * sin + y * cos);
            (-(u * u) / (2.0 * sigma.0 * sigma.0) - (v * v) / (2.0 * sigma.1 * sigma.1)).exp()
        })
        .collect()
}

fn measure(values: &[f64], radius: usize, psf_sigma: f64) -> Roundness {
    Roundness::measure(RoundnessStamp {
        values,
        radius,
        psf_sigma,
    })
    .unwrap()
}

/// photutils' `roundness2` and `roundness1` of the same stamps' DAOFIND cutouts, from its marginal
/// fit and its quadrant slices reproduced in numpy over f64. Ours are f32, so they agree to its rounding: a
/// half ulp under 1 is 6e-8.
const PHOTUTILS_F32: f32 = 1e-7;

/// A round star reads 0 on both when centred. Off the centre both read the phase, to second order
/// for SROUND: at FWHM 2, on the 5 × 5 cutout, its worst over a 21 × 21 phase grid is 0.3637, at the
/// corner (½, ½), under the default `max_roundness` of 0.5. Measured on DAOFIND's convolved samples
/// instead, the same star reads up to 0.45. Review item 10.1 found the former SROUND above 0.5 for 57% of
/// FWHM-2 stars.
#[test]
fn a_round_star_reads_only_its_phase() {
    let sigma = 2.0 / 2.354_82;
    let centred = measure(&stamp(4, (sigma, sigma), 0.0, (0.0, 0.0)), 4, sigma);
    assert!(
        centred.ground.abs() < 1e-12 && centred.sround.abs() < 1e-6,
        "{centred:?}"
    );
    for (offset, ground, sround) in [
        ((0.5, 0.0), -0.219_274_97, -0.179_775_27),
        ((0.0, 0.5), 0.219_274_97, 0.179_775_27),
        ((0.5, 0.5), 0.0, 0.363_711_33),
        ((-0.3, 0.4), 0.061_485_24, -0.138_095_76),
    ] {
        let shifted = measure(&stamp(4, (sigma, sigma), 0.0, offset), 4, sigma);
        assert!(
            (shifted.ground - ground).abs() <= PHOTUTILS_F32
                && (shifted.sround - sround).abs() <= PHOTUTILS_F32,
            "{offset:?}: {shifted:?}"
        );
    }
}

/// Elongation by 2 : 1 along x reads GROUND −0.9833 and along y +0.9833; SROUND sees an axis
/// elongation in Q₁ and Q₃ against Q₂ and Q₄, −0.3365 along x and +0.3365 along y, and a diagonal
/// one in Q₁ and Q₃ alone, 0.6230, where GROUND reads 0. Each on the 5 × 5 cutout of PSF σ 1.5. A source of both signs cancelling GROUND's
/// numerator to non-positive height has no roundness.
#[test]
fn elongation_has_its_sign_on_both_metrics() {
    for (sigma, angle, ground, sround) in [
        ((2.0, 1.0), 0.0, -0.983_267_7, -0.336_543_3),
        ((1.0, 2.0), 0.0, 0.983_267_7, 0.336_543_3),
        ((2.0, 1.0), std::f64::consts::FRAC_PI_4, 0.0, 0.623_014_3),
    ] {
        let elongated = measure(&stamp(7, sigma, angle, (0.0, 0.0)), 7, 1.5);
        assert!(
            (elongated.ground - ground).abs() <= PHOTUTILS_F32
                && (elongated.sround - sround).abs() <= PHOTUTILS_F32,
            "{sigma:?} at {angle}: {elongated:?}"
        );
    }
    let negative: Vec<f64> = stamp(4, (1.0, 1.0), 0.0, (0.0, 0.0))
        .into_iter()
        .map(|value| -value)
        .collect();
    assert!(
        Roundness::measure(RoundnessStamp {
            values: &negative,
            radius: 4,
            psf_sigma: 1.0
        })
        .is_none()
    );
}
