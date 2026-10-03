use crate::background_mesh::spline::spline_segment::SplineSegment;
use crate::internals::simd_check::{SWEEP_WIDTHS, ScalarSimd, assert_simd_matches_scalar};
use crate::star_detection::background::simd::internals::interpolate_segment_cubic_scalar;
use crate::star_detection::background::simd::{
    InterpolateSegment, SegmentRamp, interpolate_segment_cubic,
};

#[test]
#[should_panic(expected = "assertion")]
fn cubic_segment_simd_mismatched_lengths_panics() {
    // The kernel walks both outputs in lockstep, so a mismatch would leave the longer one's tail
    // unwritten: rejected in release builds too.
    let mut bg = vec![0.0f32; 8];
    let mut noise = vec![0.0f32; 4];
    interpolate_segment_cubic(
        &mut bg,
        &mut noise,
        SplineSegment {
            f0: 100.0,
            f1: 200.0,
            a: -5.0,
            b: 3.0,
        },
        SplineSegment {
            f0: 5.0,
            f1: 10.0,
            a: -0.5,
            b: 0.3,
        },
        SegmentRamp {
            start: 0.0,
            step: 0.1,
        },
    );
}

/// The absolute terms of `segment.eval(t)`, which bound its rounding error.
fn eval_magnitude(segment: SplineSegment, t: f32) -> f32 {
    let ct = 1.0 - t;
    segment.f0.abs()
        + (t * (segment.f1 - segment.f0)).abs()
        + (t * ct).abs() * (((2.0 - t) * segment.a).abs() + ((1.0 + t) * segment.b).abs())
}

/// Every tier against the scalar reference, bit for bit: the lanes evaluate `eval`'s own unfused
/// expression at the scalar's own parameter. The shape fills both segments' coefficients, and the
/// ramp runs from -0.25 to 1.25 so lanes on both ends extrapolate. Widths up to 1024 cover the
/// longest segments a tile mesh makes, over which a parameter stepped by repeated addition would
/// drift from the scalar's `start + i·step`.
#[test]
fn cubic_segment_matches_scalar() {
    let widths: Vec<usize> = SWEEP_WIDTHS.iter().copied().chain([256, 1024]).collect();
    assert_simd_matches_scalar(&widths, 0.0, |tier, shape, width| {
        let p = shape.row(8, width);
        let bg = SplineSegment {
            f0: p[0],
            f1: p[1],
            a: p[2],
            b: p[3],
        };
        let noise = SplineSegment {
            f0: p[4],
            f1: p[5],
            a: p[6],
            b: p[7],
        };
        let ramp = SegmentRamp {
            start: -0.25,
            step: 1.5 / width as f32,
        };
        let mut bg_scalar = vec![0.0f32; width];
        let mut noise_scalar = vec![0.0f32; width];
        let mut bg_simd = vec![0.0f32; width];
        let mut noise_simd = vec![0.0f32; width];
        interpolate_segment_cubic_scalar(&mut bg_scalar, &mut noise_scalar, bg, noise, ramp);
        tier.run(InterpolateSegment {
            bg_out: &mut bg_simd,
            noise_out: &mut noise_simd,
            bg,
            noise,
            ramp,
        });
        bg_scalar.extend(noise_scalar);
        bg_simd.extend(noise_simd);
        ScalarSimd::new(bg_scalar, bg_simd)
    });
}

#[test]
fn cubic_segment_simd_endpoints() {
    // At t=0 result should be f0, at t=1 result should be f1
    // (regardless of a, b coefficients, since t*(1-t) = 0 at both endpoints)
    let mut bg = vec![0.0f32; 2];
    let mut noise = vec![0.0f32; 2];

    // t=0 for first pixel, t=1 for second pixel
    interpolate_segment_cubic(
        &mut bg,
        &mut noise,
        SplineSegment {
            f0: 100.0,
            f1: 200.0,
            a: -10.0,
            b: 7.0,
        },
        SplineSegment {
            f0: 5.0,
            f1: 15.0,
            a: -1.0,
            b: 0.5,
        },
        SegmentRamp {
            start: 0.0,
            step: 1.0,
        },
    );

    // f(0) = 1*100 + 0*200 + 0*1*(1*a + 0*b) = 100
    assert!(
        (bg[0] - 100.0).abs() < 1e-4,
        "t=0: bg should be f0=100, got {}",
        bg[0]
    );
    // f(1) = 0*100 + 1*200 + 1*0*(0*a + 1*b) = 200
    assert!(
        (bg[1] - 200.0).abs() < 1e-4,
        "t=1: bg should be f1=200, got {}",
        bg[1]
    );
    assert!(
        (noise[0] - 5.0).abs() < 1e-4,
        "t=0: noise should be f0=5, got {}",
        noise[0]
    );
    assert!(
        (noise[1] - 15.0).abs() < 1e-4,
        "t=1: noise should be f1=15, got {}",
        noise[1]
    );
}

#[test]
fn cubic_segment_simd_midpoint() {
    // At t=0.5, using f(t) = f0 + t*(f1-f0) - t*ct*((2-t)*a + (1+t)*b):
    //   = 0.5*f0 + 0.5*f1 - 0.5*0.5*(1.5*a + 1.5*b)
    //   = (f0+f1)/2 - 0.375*(a+b)
    let mut bg = vec![0.0f32; 1];
    let mut noise = vec![0.0f32; 1];

    // Expected: (100+200)/2 - 0.375*(-8+16) = 150 - 3 = 147
    interpolate_segment_cubic(
        &mut bg,
        &mut noise,
        SplineSegment {
            f0: 100.0,
            f1: 200.0,
            a: -8.0,
            b: 16.0,
        },
        SplineSegment {
            f0: 0.0,
            f1: 0.0,
            a: 0.0,
            b: 0.0,
        },
        SegmentRamp {
            start: 0.5,
            step: 1.0,
        },
    );

    assert!(
        (bg[0] - 147.0).abs() < 1e-4,
        "Midpoint: expected 147, got {}",
        bg[0]
    );
}

#[test]
fn cubic_segment_simd_linear_when_no_correction() {
    // With a=0, b=0, cubic spline reduces to linear interpolation
    let mut bg = vec![0.0f32; 50];
    let mut noise = vec![0.0f32; 50];

    let f0 = 100.0;
    let f1 = 200.0;
    let ramp = SegmentRamp {
        start: 0.0,
        step: 1.0 / 49.0,
    };

    interpolate_segment_cubic(
        &mut bg,
        &mut noise,
        SplineSegment {
            f0,
            f1,
            a: 0.0,
            b: 0.0,
        },
        SplineSegment {
            f0: 5.0,
            f1: 10.0,
            a: 0.0,
            b: 0.0,
        },
        ramp,
    );

    for (i, &b) in bg.iter().enumerate() {
        let t = i as f32 * ramp.step;
        let expected = (1.0 - t) * f0 + t * f1;
        assert!(
            (b - expected).abs() < 1e-3,
            "i={i}: expected linear {expected}, got {b}"
        );
    }
}

/// Past the knots the segment's cubic runs on: at t = −0.5, `1.5·100 − 0.5·200 + 0.75·(2.5·−5 +
/// 0.5·3)` = 41.75, and at t = 1.3, `−0.3·100 + 1.3·200 + 0.39·(0.7·−5 + 2.3·3)` = 231.326, each to
/// the seven roundings of `eval` over its terms' magnitude.
#[test]
fn cubic_segment_simd_extrapolates_past_the_knots() {
    let mut bg = vec![0.0f32; 10];
    let mut noise = vec![0.0f32; 10];
    let segment = SplineSegment {
        f0: 100.0,
        f1: 200.0,
        a: -5.0,
        b: 3.0,
    };
    let ramp = SegmentRamp {
        start: -0.5,
        step: 0.2,
    };
    interpolate_segment_cubic(&mut bg, &mut noise, segment, segment, ramp);

    for (i, expected) in [(0, 41.75f32), (9, 231.326)] {
        let bound = 7.0 * f32::EPSILON * eval_magnitude(segment, ramp.t_at(i));
        assert!(
            (bg[i] - expected).abs() <= bound && (noise[i] - expected).abs() <= bound,
            "t = {}: {} and {} against {expected} ± {bound}",
            ramp.t_at(i),
            bg[i],
            noise[i]
        );
    }
}
