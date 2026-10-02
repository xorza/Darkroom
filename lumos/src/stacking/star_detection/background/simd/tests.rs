use crate::stacking::star_detection::background::simd::*;
use crate::testing::prelude::*;

#[test]
#[should_panic(expected = "assertion")]
fn cubic_segment_simd_mismatched_lengths_panics() {
    // Every SIMD backend derives its store bound solely from bg_out.len() and writes
    // into noise_out with that same bound — a mismatch must be rejected even in release
    // builds, not just debug, since it would otherwise be an out-of-bounds write.
    let mut bg = vec![0.0f32; 8];
    let mut noise = vec![0.0f32; 4];
    interpolate_segment_cubic_simd(
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

type SegmentFn = unsafe fn(&mut [f32], &mut [f32], SplineSegment, SplineSegment, SegmentRamp);

const BACKENDS: &[Backend<SegmentFn>] = &[
    #[cfg(target_arch = "x86_64")]
    Backend::new(SimdTier::Avx2Fma, avx2::interpolate_segment_cubic_avx2),
    #[cfg(target_arch = "x86_64")]
    Backend::new(SimdTier::Sse41, sse41::interpolate_segment_cubic_sse),
    #[cfg(target_arch = "aarch64")]
    Backend::new(SimdTier::Neon, neon::interpolate_segment_cubic_neon),
];

/// The absolute terms of `segment.eval(t)`, which the backends may round differently.
fn eval_magnitude(segment: SplineSegment, t: f32) -> f32 {
    let ct = 1.0 - t;
    (ct * segment.f0).abs()
        + (t * segment.f1).abs()
        + (t * ct).abs() * (((2.0 - t) * segment.a).abs() + ((1.0 + t) * segment.b).abs())
}

/// Every backend against the scalar reference. The shape fills both segments' coefficients, and
/// the ramp runs from -0.25 to 1.25 so lanes on both ends clamp. Widths up to 1024 cover the
/// longest segments a tile mesh makes, over which a parameter stepped by repeated addition would
/// drift from the scalar's `start + i·step`.
///
/// A term of `eval` passes through at most seven roundings (`1 - t`, `t·ct`, `2 - t`, the
/// product with `a`, the sum with the `b` term, the product with `t·ct`, the final difference),
/// so the paths differ by the sum bound for seven.
#[test]
fn cubic_segment_backends_match_scalar() {
    let widths: Vec<usize> = SWEEP_WIDTHS.iter().copied().chain([256, 1024]).collect();
    assert_simd_matches_scalar(
        BACKENDS,
        &widths,
        ScalarSimd::sum_tolerance(7),
        |kernel, shape, width| {
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
            // SAFETY: the harness runs only backends whose tier this CPU has, and the two
            // outputs have one length.
            unsafe { kernel(&mut bg_simd, &mut noise_simd, bg, noise, ramp) };
            let magnitude = (0..width)
                .map(|i| eval_magnitude(bg, ramp.t_at(i)))
                .chain((0..width).map(|i| eval_magnitude(noise, ramp.t_at(i))))
                .collect();
            bg_scalar.extend(noise_scalar);
            bg_simd.extend(noise_simd);
            ScalarSimd::of_sums(bg_scalar, bg_simd, magnitude)
        },
    );
}

#[test]
fn cubic_segment_simd_endpoints() {
    // At t=0 result should be f0, at t=1 result should be f1
    // (regardless of a, b coefficients, since t*(1-t) = 0 at both endpoints)
    let mut bg = vec![0.0f32; 2];
    let mut noise = vec![0.0f32; 2];

    // t=0 for first pixel, t=1 for second pixel
    interpolate_segment_cubic_simd(
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
    // At t=0.5, using f(t) = ct*f0 + t*f1 - t*ct*((2-t)*a + (1+t)*b):
    //   = 0.5*f0 + 0.5*f1 - 0.5*0.5*(1.5*a + 1.5*b)
    //   = (f0+f1)/2 - 0.375*(a+b)
    let mut bg = vec![0.0f32; 1];
    let mut noise = vec![0.0f32; 1];

    // Expected: (100+200)/2 - 0.375*(-8+16) = 150 - 3 = 147
    interpolate_segment_cubic_simd(
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

    interpolate_segment_cubic_simd(
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
        let t = (i as f32 * ramp.step).clamp(0.0, 1.0);
        let expected = (1.0 - t) * f0 + t * f1;
        assert!(
            (b - expected).abs() < 1e-3,
            "i={i}: expected linear {expected}, got {b}"
        );
    }
}

#[test]
fn cubic_segment_simd_clamping() {
    // t values outside [0,1] should be clamped
    let mut bg = vec![0.0f32; 10];
    let mut noise = vec![0.0f32; 10];

    // tx_start = -0.5, step = 0.2 → t goes from -0.5 to 1.3
    interpolate_segment_cubic_simd(
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
            start: -0.5,
            step: 0.2,
        },
    );

    // First element: t = -0.5 clamped to 0 → bg = f0 = 100
    assert!(
        (bg[0] - 100.0).abs() < 1e-4,
        "t<0 clamped: expected f0=100, got {}",
        bg[0]
    );

    // Last element: t = -0.5 + 9*0.2 = 1.3 clamped to 1 → bg = f1 = 200
    assert!(
        (bg[9] - 200.0).abs() < 1e-4,
        "t>1 clamped: expected f1=200, got {}",
        bg[9]
    );
}
