use crate::bg_mesh::mesh_surface::simd::{InterpolateSegment, SegmentRamp};
use crate::bg_mesh::spline::spline_segment::SplineSegment;
use crate::internals::simd_check::{SWEEP_WIDTHS, ScalarSimd, assert_simd_matches_scalar};
use crate::simd::Kernel;

/// The absolute terms of `segment.eval(t)`, which bound its rounding error.
fn eval_magnitude(segment: SplineSegment, t: f32) -> f32 {
    let ct = 1.0 - t;
    segment.f0.abs()
        + (t * (segment.f1 - segment.f0)).abs()
        + (t * ct).abs() * (((2.0 - t) * segment.a).abs() + ((1.0 + t) * segment.b).abs())
}

/// Every tier against the scalar reference, bit for bit: the lanes evaluate `eval`'s own unfused
/// expression at the scalar's own parameter. The ramp runs from -0.25 to 1.25 so lanes on both
/// ends extrapolate. Widths up to 1024 cover the longest segments a tile mesh makes, over which a
/// parameter stepped by repeated addition would drift from the scalar's `start + i·step`.
#[test]
fn cubic_segment_matches_scalar() {
    let widths: Vec<usize> = SWEEP_WIDTHS.iter().copied().chain([256, 1024]).collect();
    assert_simd_matches_scalar(&widths, 0.0, |tier, shape, width| {
        let p = shape.row(4, width);
        let segment = SplineSegment {
            f0: p[0],
            f1: p[1],
            a: p[2],
            b: p[3],
        };
        let ramp = SegmentRamp {
            start: -0.25,
            step: 1.5 / width as f32,
        };
        let mut scalar = vec![0.0f32; width];
        let mut simd = vec![0.0f32; width];
        InterpolateSegment {
            out: &mut scalar,
            segment,
            ramp,
        }
        .run_scalar();
        tier.run(InterpolateSegment {
            out: &mut simd,
            segment,
            ramp,
        });
        ScalarSimd::new(scalar, simd)
    });
}

/// At t = 0 a segment is `f0` and at t = 1 `f1`, whatever its `a` and `b`, since `t·(1 − t)` is 0
/// at both.
#[test]
fn cubic_segment_simd_endpoints() {
    let mut out = vec![0.0f32; 2];
    InterpolateSegment {
        out: &mut out,
        segment: SplineSegment {
            f0: 100.0,
            f1: 200.0,
            a: -10.0,
            b: 7.0,
        },
        ramp: SegmentRamp {
            start: 0.0,
            step: 1.0,
        },
    }
    .dispatch();
    assert_eq!(out, [100.0, 200.0]);
}

/// At t = 0.5, `f0/2 + f1/2 − 0.25·(1.5·a + 1.5·b)` = 150 − 0.375·(−8 + 16) = 147.
#[test]
fn cubic_segment_simd_midpoint() {
    let mut out = vec![0.0f32; 1];
    InterpolateSegment {
        out: &mut out,
        segment: SplineSegment {
            f0: 100.0,
            f1: 200.0,
            a: -8.0,
            b: 16.0,
        },
        ramp: SegmentRamp {
            start: 0.5,
            step: 1.0,
        },
    }
    .dispatch();
    assert!((out[0] - 147.0).abs() < 1e-4, "midpoint: {}", out[0]);
}

/// With `a = b = 0` the segment is the line from `f0` to `f1`.
#[test]
fn cubic_segment_simd_linear_when_no_correction() {
    let mut out = vec![0.0f32; 50];
    let (f0, f1) = (100.0, 200.0);
    let ramp = SegmentRamp {
        start: 0.0,
        step: 1.0 / 49.0,
    };
    InterpolateSegment {
        out: &mut out,
        segment: SplineSegment {
            f0,
            f1,
            a: 0.0,
            b: 0.0,
        },
        ramp,
    }
    .dispatch();
    for (i, &value) in out.iter().enumerate() {
        let t = i as f32 * ramp.step;
        let expected = (1.0 - t) * f0 + t * f1;
        assert!(
            (value - expected).abs() < 1e-3,
            "i={i}: {expected} vs {value}"
        );
    }
}

/// Past the knots the segment's cubic runs on: at t = −0.5, `1.5·100 − 0.5·200 + 0.75·(2.5·−5 +
/// 0.5·3)` = 41.75, and at t = 1.3, `−0.3·100 + 1.3·200 + 0.39·(0.7·−5 + 2.3·3)` = 231.326, each to
/// the seven roundings of `eval` over its terms' magnitude.
#[test]
fn cubic_segment_simd_extrapolates_past_the_knots() {
    let mut out = vec![0.0f32; 10];
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
    InterpolateSegment {
        out: &mut out,
        segment,
        ramp,
    }
    .dispatch();
    for (i, expected) in [(0, 41.75f32), (9, 231.326)] {
        let bound = 7.0 * f32::EPSILON * eval_magnitude(segment, ramp.t_at(i));
        assert!(
            (out[i] - expected).abs() <= bound,
            "t = {}: {} against {expected} ± {bound}",
            ramp.t_at(i),
            out[i]
        );
    }
}
