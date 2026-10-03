use std::f64::consts::PI;

use crate::internals::prelude::*;
use crate::registration::config::{self, InterpolationMethod};
use crate::registration::distortion::sip::{SipConfig, SipPolynomial};
use crate::registration::resample::kernel::{LanczosOrder, internals};
use crate::registration::resample::row;
use crate::registration::resample::row_positions::RowPositions;
use crate::registration::transform::{Transform, WarpTransform};

const LANCZOS: [InterpolationMethod; 3] = [
    InterpolationMethod::Lanczos2,
    InterpolationMethod::Lanczos3,
    InterpolationMethod::Lanczos4,
];

/// Output row `y` of `input` warped by `transform` with `method`, border `border_value`.
fn warp_row(
    input: &Buffer2<f32>,
    y: usize,
    transform: &WarpTransform,
    method: InterpolationMethod,
    border_value: f32,
) -> Vec<f32> {
    let size = Size2us::new(input.width(), input.height());
    let mut positions = RowPositions::default();
    positions.fill(y, size.width, transform, size);
    let mut output = vec![f32::NAN; size.width];
    row::sample_row(
        input,
        positions.positions(),
        method,
        border_value,
        &mut output,
    );
    output
}

/// A `size` image of signed, unstructured values.
fn signed_field(size: Size2us) -> Buffer2<f32> {
    Buffer2::new(
        size.width,
        size.height,
        (0..size.pixel_count())
            .map(|i| ((i * 13 + i / size.width * 7) % 31) as f32 / 9.0 - 1.7)
            .collect(),
    )
}

/// A SIP correction of a mild radial field over `size`, on `transform`.
fn sip_warp(size: Size2us, transform: Transform) -> WarpTransform {
    let center = DVec2::new(size.width as f64 / 2.0, size.height as f64 / 2.0);
    let mut reference = Vec::new();
    for y in 0..size.height {
        for x in 0..size.width {
            reference.push(DVec2::new(x as f64, y as f64));
        }
    }
    let target: Vec<DVec2> = reference
        .iter()
        .map(|&r| {
            let d = r - center;
            transform.apply(r + d * 1e-4 * d.length_squared())
        })
        .collect();
    let config = SipConfig {
        order: 3,
        reference_point: Some(center),
        ..SipConfig::default()
    };
    let fit = SipPolynomial::fit_from_transform(&reference, &target, &transform, &config).unwrap();
    WarpTransform::with_sip(transform, fit.polynomial)
}

/// Every method on a row equals the single-point oracle at that pixel's position, across a
/// translation, a rotation with scale, a homography and a SIP correction, on the top, middle and
/// bottom rows of several image sizes — the narrow ones reach the edge on every row.
///
/// The oracle evaluates each method from its definition. Nearest, bilinear and bicubic sample the
/// same position with the same arithmetic, so they agree exactly. Lanczos sums the same taps, but
/// the vector kernel sums them in another order and the row multiplies by the reciprocal of the
/// tap total where the oracle divides: `SIZE² + 3` roundings against the window's absolute sum.
#[test]
fn sample_row_matches_the_single_point_oracle() {
    let transforms = |size: Size2us| {
        [
            WarpTransform::new(Transform::translation(DVec2::new(0.37, -1.43))),
            WarpTransform::new(Transform::similarity(DVec2::new(1.5, -0.75), 0.07, 1.03)),
            WarpTransform::new(Transform::homography([
                1.01, 0.02, 0.5, -0.015, 0.99, 0.75, 2e-3, -1e-3,
            ])),
            sip_warp(
                size,
                Transform::similarity(DVec2::new(0.25, 0.5), -0.04, 0.98),
            ),
        ]
    };
    for size in [
        Size2us::new(9, 7),
        Size2us::new(24, 20),
        Size2us::new(41, 33),
    ] {
        let input = signed_field(size);
        for transform in &transforms(size) {
            for method in InterpolationMethod::ALL {
                let params = config::internals::warp_params(method);
                for y in [0, size.height / 2, size.height - 1] {
                    let row = warp_row(&input, y, transform, method, params.border_value);
                    for (x, &actual) in row.iter().enumerate() {
                        let source = transform.apply(DVec2::new(x as f64, y as f64));
                        let expected = internals::interpolate(&input, source, params);
                        let taps = match method {
                            InterpolationMethod::Lanczos2 => 16,
                            InterpolationMethod::Lanczos3 => 36,
                            InterpolationMethod::Lanczos4 => 64,
                            _ => 0,
                        };
                        // The window's absolute sum is at most its taps times the largest sample
                        // (1.7) times the largest product of normalized weights (1).
                        let bound = (taps + 3) as f32 * f32::EPSILON * taps as f32 * 1.7;
                        assert!(
                            (actual - expected).abs() <= bound,
                            "{method:?} {size:?} ({x}, {y}): row {actual}, oracle {expected}"
                        );
                    }
                }
            }
        }
    }
}

/// One column of ones, sampled a quarter and a half pixel past it, against hand values. Each
/// method has its own, so a method wired to another's kernel fails here.
///
/// The column is constant down the image, so the y weights divide out and the value is the x
/// weight of the column's tap over the sum of the taps. Nearest at 4.25 is the column, 1, and at
/// 4.5 rounds up past it, 0; bilinear is `1 − f`. Catmull-Rom gives the column `K(f)` of weights
/// summing to 1: `K(0.25)` = 0.8671875 and `K(0.5)` = 0.5625. Lanczos-`a` at 4.5 has its taps at
/// distances ½, 1½, …, `a − ½` in pairs, and with `s₁ = sin(π/8)`, `s₃ = sin(3π/8)`:
/// - a = 2: `L(½) = 4√2/π²`, `L(1½) = −4√2/(9π²)`, so `1/(2(1 − 1/9))` = 9/16 — Catmull-Rom's
///   value at ½, which is why bicubic is checked at 4.25 too;
/// - a = 3: `L(½) = 6/π²`, `L(1½) = −4/(3π²)`, `L(2½) = 6/(25π²)`, so `3/(6 − 4/3 + 6/25)` =
///   225/368;
/// - a = 4: `L(k + ½) ∝ ±s/(2k + 1)²` with `s` = s₁, s₃, s₃, s₁, so
///   `s₁/(2(s₁·48/49 − s₃·16/225))`.
///
/// The table entries at these distances are the f32 kernel, each off by 1e-6 at most (see
/// `math::lanczos`'s tests); the ratio moves by that for its numerator and by up to `2a` of them
/// for its sum, which is near 1. The 64 products and the sum round to 64·ε more.
#[test]
fn a_column_of_ones_samples_to_hand_values() {
    let size = Size2us::new(10, 9);
    let input = Buffer2::new(
        size.width,
        size.height,
        (0..size.pixel_count())
            .map(|i| if i % size.width == 4 { 1.0 } else { 0.0 })
            .collect(),
    );
    let s1 = (PI / 8.0).sin();
    let s3 = (3.0 * PI / 8.0).sin();
    for (method, shift, expected) in [
        (InterpolationMethod::Nearest, 0.25, 1.0),
        (InterpolationMethod::Nearest, 0.5, 0.0),
        (InterpolationMethod::Bilinear, 0.25, 0.75),
        (InterpolationMethod::Bilinear, 0.5, 0.5),
        (InterpolationMethod::Bicubic, 0.25, 0.867_187_5),
        (InterpolationMethod::Bicubic, 0.5, 0.5625),
        (InterpolationMethod::Lanczos2, 0.5, 9.0 / 16.0),
        (InterpolationMethod::Lanczos3, 0.5, 225.0 / 368.0),
        (
            InterpolationMethod::Lanczos4,
            0.5,
            s1 / (2.0 * (s1 * 48.0 / 49.0 - s3 * 16.0 / 225.0)),
        ),
    ] {
        let tolerance = LanczosOrder::of(method).map_or(0.0, |order| {
            (2 * order.a() + 1) as f64 * 1e-6 + 64.0 * f64::from(f32::EPSILON)
        });
        let shift = WarpTransform::new(Transform::translation(DVec2::new(shift, 0.0)));
        let actual = f64::from(warp_row(&input, 4, &shift, method, 0.0)[4]);
        assert!(
            (actual - expected).abs() <= tolerance,
            "{method:?} {shift:?}: {actual}, expected {expected}"
        );
        let params = config::internals::warp_params(method);
        let oracle = f64::from(internals::interpolate(
            &input,
            shift.apply(DVec2::new(4.0, 4.0)),
            params,
        ));
        assert!(
            (oracle - expected).abs() <= tolerance,
            "{method:?} oracle: {oracle}, expected {expected}"
        );
    }
}

/// A row shifted half off the source takes the border where its positions leave the footprint and
/// samples inside it: a translation by 1.75 moves output x to source x + 1.75, which passes the
/// last pixel's half-pixel rim (3.5 on a 4-wide image) from x = 2 on.
#[test]
fn positions_outside_the_footprint_take_the_border() {
    let input = Buffer2::new(4, 1, vec![10.0, 20.0, 30.0, 40.0]);
    let shift = WarpTransform::new(Transform::translation(DVec2::new(1.75, 0.0)));
    let row = warp_row(&input, 0, &shift, InterpolationMethod::Bilinear, -5.0);
    // x = 0 → 1.75: 20 + 0.75·10 = 27.5; x = 1 → 2.75: 30 + 0.75·10 = 37.5.
    assert_eq!(row, [27.5, 37.5, -5.0, -5.0]);
}

/// A constant reproduces itself to rounding, in the interior and at the edges, where bicubic drops
/// its outside taps and Lanczos falls back to bilinear: the normalized weights sum to one, so only
/// the f32 summation of up to 64 terms of size 2.5 is left, ≈ 64·ε·2.5 = 1.9e-5.
#[test]
fn every_method_preserves_signed_constants_at_interior_and_edges() {
    let size = Size2us::new(24, 20);
    // Both stay inside the footprint: the shift moves no position more than 0.43 px.
    let transforms = [
        WarpTransform::new(Transform::identity()),
        WarpTransform::new(Transform::translation(DVec2::new(0.37, -0.43))),
    ];
    for method in InterpolationMethod::ALL {
        for transform in &transforms {
            for expected in [-1.25, 0.0, 2.5] {
                let input = Buffer2::new_filled(size.width, size.height, expected);
                for y in [0, 1, 4, 10, size.height - 1] {
                    for (x, actual) in warp_row(&input, y, transform, method, 0.0)
                        .into_iter()
                        .enumerate()
                    {
                        assert!(
                            (actual - expected).abs() < 2e-5,
                            "{method:?} ({x}, {y}): expected {expected}, got {actual}"
                        );
                    }
                }
            }
        }
    }
}

/// Adding a constant to every pixel adds it to every output, for the same reason: the offset
/// passes through normalized weights, and 5e-5 holds the f32 sums of the two warps.
#[test]
fn lanczos_is_translation_invariant_for_signed_data() {
    let size = Size2us::new(24, 20);
    let input = signed_field(size);
    let offset = 2.25;
    let shifted = Buffer2::new(
        size.width,
        size.height,
        input.pixels().iter().map(|value| value + offset).collect(),
    );
    let inverse = WarpTransform::new(Transform::translation(DVec2::new(0.37, -0.43)).inverse());
    for method in LANCZOS {
        for y in [0, 1, 5, 10, size.height - 1] {
            let output = warp_row(&input, y, &inverse, method, 0.0);
            let shifted_output = warp_row(&shifted, y, &inverse, method, 0.0);
            for x in 1..size.width {
                let actual_offset = shifted_output[x] - output[x];
                assert!(
                    (actual_offset - offset).abs() < 5e-5,
                    "{method:?} ({x}, {y}): expected offset {offset}, got {actual_offset}"
                );
            }
        }
    }
}
