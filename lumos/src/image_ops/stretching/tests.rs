use super::*;
use crate::image_ops::deinterleave_f32;
use crate::image_ops::op::OpError;
use crate::math::statistics::median_f32_mut;
use imaginarium::{Buffer2, DeinterleavedImageData, Image};

fn gray(width: usize, height: usize, px: Vec<f32>) -> Image {
    Image::from(&DeinterleavedImageData::from_channels([Buffer2::new(
        width, height, px,
    )]))
}

fn rgb(width: usize, height: usize, r: Vec<f32>, g: Vec<f32>, b: Vec<f32>) -> Image {
    Image::from(&DeinterleavedImageData::from_channels([
        Buffer2::new(width, height, r),
        Buffer2::new(width, height, g),
        Buffer2::new(width, height, b),
    ]))
}

/// Channel `c` of an image as a buffer (for assertions).
fn channel(image: &Image, c: usize) -> Buffer2<f32> {
    deinterleave_f32(image)[c].clone()
}

fn median_of(v: &[f32]) -> f32 {
    let mut c = v.to_vec();
    median_f32_mut(&mut c)
}

// ---- Midtones Transfer Function ----

#[test]
fn mtf_fixed_points_identity_and_direction() {
    for &m in &[0.1f32, 0.25, 0.5, 0.75, 0.9] {
        assert_eq!(mtf(m, 0.0), 0.0, "MTF(m,0) = 0");
        assert_eq!(mtf(m, 1.0), 1.0, "MTF(m,1) = 1");
        assert!((mtf(m, m) - 0.5).abs() < 1e-6, "MTF(m,m) = 0.5 for m={m}");
    }
    // m = 0.5 is the identity (exactly, since 2m-1 = 0).
    for &x in &[0.1f32, 0.3, 0.7, 0.9] {
        assert!((mtf(0.5, x) - x).abs() < 1e-6, "MTF(0.5,·) is the identity");
    }
    // m < 0.5 brightens, m > 0.5 darkens — hand-computed at x = 0.25.
    assert!(
        (mtf(0.25, 0.25) - 0.5).abs() < 1e-6,
        "m<0.5 brightens: 0.25 -> 0.5"
    );
    assert!(
        (mtf(0.75, 0.25) - 0.1).abs() < 1e-6,
        "m>0.5 darkens: 0.25 -> 0.1"
    );
}

#[test]
fn mtf_monotonic_increasing() {
    for &m in &[0.1f32, 0.3, 0.5, 0.7, 0.9] {
        let mut prev = f32::NEG_INFINITY;
        for i in 0..=100 {
            let y = mtf(m, i as f32 / 100.0);
            assert!(y >= prev - 1e-7, "MTF must be monotonic (m={m})");
            prev = y;
        }
    }
}

#[test]
fn mtf_self_inverse_identity() {
    // The midtones balance that maps x0 -> t is MTF(t, x0), so MTF(MTF(t, x0), x0) = t.
    for &x0 in &[0.02f32, 0.05, 0.1, 0.3] {
        for &t in &[0.1f32, 0.25, 0.4] {
            let m = mtf(t, x0);
            assert!(
                (mtf(m, x0) - t).abs() < 1e-5,
                "self-inverse (x0={x0}, t={t})"
            );
        }
    }
}

// ---- normalized arcsinh ----

#[test]
fn asinh_endpoints_and_monotonic() {
    for &beta in &[0.01f32, 0.1, 1.0, 10.0] {
        let c = AsinhCurve::new(beta);
        assert!(c.eval(0.0).abs() < 1e-7, "f(0) = 0 (beta={beta})");
        assert!((c.eval(1.0) - 1.0).abs() < 1e-6, "f(1) = 1 (beta={beta})");
        let mut prev = f32::NEG_INFINITY;
        for i in 0..=100 {
            let y = c.eval(i as f32 / 100.0);
            assert!(
                y >= prev - 1e-7,
                "asinh stretch must be monotonic (beta={beta})"
            );
            prev = y;
        }
    }
}

#[test]
fn asinh_beta_controls_strength() {
    // Smaller beta = stronger stretch: a faint value is lifted higher.
    let aggressive = AsinhCurve::new(0.01).eval(0.05);
    let gentle = AsinhCurve::new(1.0).eval(0.05);
    assert!(
        aggressive > gentle,
        "smaller beta lifts faint signal more ({aggressive} vs {gentle})"
    );
    // Large beta approaches the identity.
    assert!(
        (AsinhCurve::new(1000.0).eval(0.5) - 0.5).abs() < 1e-2,
        "large beta ~ identity"
    );
    // Hand-computed: beta=0.01, f(0.1) = asinh(10)/asinh(100) = 2.99822/5.29842 = 0.56587.
    assert!((AsinhCurve::new(0.01).eval(0.1) - 0.56587).abs() < 2e-3);
}

#[test]
fn solve_beta_hits_target_background() {
    for &(median, target) in &[(0.05f32, 0.2f32), (0.1, 0.25), (0.02, 0.15)] {
        let beta = solve_asinh_beta(median, target);
        let got = AsinhCurve::new(beta).eval(median);
        assert!(
            (got - target).abs() < 2e-3,
            "median {median} -> {got}, want {target}"
        );
    }
}

// ---- STF parameter derivation ----

#[test]
fn stf_params_hand_computed() {
    // median=0.1, sigma=0.02, shadow_sigmas=1.0, target=0.25:
    //   black = 0.1 - 0.02 = 0.08
    //   rescaled median = (0.1-0.08)/(1-0.08) = 0.0217391
    //   midtones = MTF(0.25, 0.0217391) = 0.0625
    //   eval(0.1) = MTF(0.0625, 0.0217391) = 0.25   (self-inverse: median maps to target)
    let c = StfCurve::new(0.1, 0.02, 1.0, 0.25);
    assert!((c.black - 0.08).abs() < 1e-6, "black = {}", c.black);
    assert!(
        (c.inv_range - 1.0 / 0.92).abs() < 1e-5,
        "inv_range = {}",
        c.inv_range
    );
    assert!(
        (c.midtones - 0.0625).abs() < 1e-5,
        "midtones = {}",
        c.midtones
    );
    assert!(
        (c.eval(0.1) - 0.25).abs() < 1e-5,
        "median maps to target background"
    );
}

#[test]
fn stf_shadow_sigmas_lower_the_black_point() {
    let b1 = StfCurve::new(0.1, 0.02, 1.0, 0.25).black;
    let b3 = StfCurve::new(0.1, 0.02, 3.0, 0.25).black;
    assert!((b1 - 0.08).abs() < 1e-6);
    assert!((b3 - 0.04).abs() < 1e-6);
    assert!(b3 < b1, "more shadow sigmas => lower black point");
}

// ---- Generalized Hyperbolic Stretch ----

#[test]
fn ghs_endpoints_and_monotonic_across_b_family() {
    // Every b case (b=-1 log, b=0 exp, b>0 hyperbolic, general b<0) must map 0->0, 1->1, monotone.
    for &b in &[-2.0f32, -1.4, -1.0, -0.3, 0.0, 0.5, 1.0, 3.0] {
        for &d in &[0.5f32, 2.0, 6.0] {
            let c = GhsCurve::new(d, b, 0.3, 0.0, 1.0);
            assert!(c.eval(0.0).abs() < 1e-6, "f(0)=0 (b={b}, d={d})");
            assert!((c.eval(1.0) - 1.0).abs() < 1e-5, "f(1)=1 (b={b}, d={d})");
            let mut prev = f32::NEG_INFINITY;
            for i in 0..=200 {
                let y = c.eval(i as f32 / 200.0);
                assert!(y >= prev - 1e-6, "monotonic (b={b}, d={d})");
                prev = y;
            }
        }
    }
}

#[test]
fn ghs_identity_when_d_zero() {
    let c = GhsCurve::new(0.0, 1.0, 0.3, 0.1, 0.9);
    for &x in &[0.0f32, 0.05, 0.3, 0.5, 0.9, 1.0] {
        assert!((c.eval(x) - x).abs() < 1e-6, "d=0 is the identity at {x}");
    }
}

#[test]
fn ghs_exponential_b0_hand_computed() {
    // b=0, sp=lp=0, hp=1, D=2 reduces to f(x) = (1 - e^(-2x)) / (1 - e^(-2)).
    let c = GhsCurve::new(2.0, 0.0, 0.0, 0.0, 1.0);
    // f(0.5) = (1 - e^-1)/(1 - e^-2) = 0.632121/0.864665 = 0.731060.
    assert!(
        (c.eval(0.5) - 0.731060).abs() < 1e-4,
        "f(0.5) = {}",
        c.eval(0.5)
    );
    // f(0.25) = (1 - e^-0.5)/(1 - e^-2) = 0.393469/0.864665 = 0.455056.
    assert!(
        (c.eval(0.25) - 0.455056).abs() < 1e-4,
        "f(0.25) = {}",
        c.eval(0.25)
    );
}

#[test]
fn ghs_protection_tails_are_linear() {
    let c = GhsCurve::new(3.0, 1.0, 0.5, 0.2, 0.8);
    // f(0)=0 and the [0, lp] segment is linear, so f(lp/2) = 0.5·f(lp).
    assert!(
        (c.eval(0.1) - 0.5 * c.eval(0.2)).abs() < 1e-4,
        "shadow tail linear from the origin"
    );
    // f(1)=1 and the [hp, 1] segment is linear, so f(0.9) = (f(0.8) + 1)/2.
    assert!(
        (c.eval(0.9) - 0.5 * (c.eval(0.8) + 1.0)).abs() < 1e-4,
        "highlight tail linear to white"
    );
}

#[test]
fn ghs_continuous_at_breakpoints() {
    // C¹ construction => no jumps at lp, sp, hp (b=-1.4 ~ asinh exercises the general b<0 form).
    let c = GhsCurve::new(2.5, -1.4, 0.4, 0.15, 0.85);
    for &bp in &[0.15f32, 0.4, 0.85] {
        let (below, above) = (c.eval(bp - 1e-3), c.eval(bp + 1e-3));
        assert!(
            (above - below).abs() < 1e-2,
            "continuous at {bp}: {below} vs {above}"
        );
    }
}

#[test]
fn ghs_clamps_out_of_range_input() {
    let c = GhsCurve::new(2.0, 1.0, 0.3, 0.0, 0.9);
    assert_eq!(
        c.eval(5.0),
        1.0,
        "above-1 input (a bright star) clamps to white"
    );
    assert_eq!(c.eval(-2.0), 0.0, "negative input clamps to black");
}

#[test]
fn ghs_d_controls_strength() {
    // With the symmetry point at the faint-signal level, stronger d lifts signal above sp higher
    // (below sp the antisymmetric curve instead compresses toward black).
    let weak = GhsCurve::new(1.0, 0.0, 0.1, 0.0, 1.0).eval(0.2);
    let strong = GhsCurve::new(6.0, 0.0, 0.1, 0.0, 1.0).eval(0.2);
    assert!(
        strong > weak,
        "stronger d lifts signal above sp more ({strong} > {weak})"
    );
}

#[test]
fn ghs_end_to_end_lifts_background_and_stays_in_range() {
    let mut px: Vec<f32> = (0..90).map(|i| 0.04 + (i % 3) as f32 * 0.01).collect();
    px.extend(std::iter::repeat_n(0.8f32, 10));
    let mut img = gray(10, 10, px.clone());
    Stretch::ghs(5.0, 0.0, 0.1).apply(&mut img).unwrap();
    let out = channel(&img, 0).to_vec();
    for &v in &out {
        assert!((0.0..=1.0).contains(&v), "output in [0,1]: {v}");
    }
    assert!(median_of(&out) > median_of(&px), "background lifted");
    assert!(out[95] > out[0], "stars stay brighter than the background");
}

// ---- color preservation ----

#[test]
fn color_preserving_keeps_channel_ratio_and_caps_highlights() {
    // Two pixels with a 2:1:1 R:G:B ratio; pixel 1 is bright enough to trip the highlight guard.
    let mut img = rgb(2, 1, vec![0.3, 0.9], vec![0.15, 0.45], vec![0.15, 0.45]);
    let cfg = Stretch {
        method: StretchMethod::Asinh { beta: 0.05 },
        color: ColorMode::ColorPreserving,
    };
    cfg.apply(&mut img).unwrap();
    let r = channel(&img, 0).to_vec();
    let g = channel(&img, 1).to_vec();
    let b = channel(&img, 2).to_vec();
    // Pixel 0: ratio preserved, below the white point.
    assert!(
        (r[0] / g[0] - 2.0).abs() < 1e-3,
        "R:G ratio preserved (px0)"
    );
    assert!((g[0] - b[0]).abs() < 1e-6, "G == B (px0)");
    assert!(r[0] < 1.0, "px0 not clipped");
    // Pixel 1: the guard caps the brightest channel at 1 but keeps the ratio exactly.
    assert!(
        (r[1] / g[1] - 2.0).abs() < 1e-3,
        "R:G ratio preserved through the guard (px1)"
    );
    let max1 = r[1].max(g[1]).max(b[1]);
    assert!(
        (max1 - 1.0).abs() < 1e-4,
        "brightest channel capped at 1, got {max1}"
    );
}

#[test]
fn per_channel_neutralizes_color_preserving_keeps_it() {
    // Background that is redder (R≈0.20) than green/blue (≈0.05), plus one white star.
    let r = vec![0.20, 0.21, 0.19, 0.20, 0.50];
    let g = vec![0.05, 0.06, 0.04, 0.05, 0.50];
    let b = vec![0.05, 0.06, 0.04, 0.05, 0.50];
    let mut linked = rgb(5, 1, r.clone(), g.clone(), b.clone());
    let mut unlinked = rgb(5, 1, r, g, b);

    Stretch::auto_stf().apply(&mut linked).unwrap();
    Stretch {
        method: StretchMethod::AutoStf {
            shadow_sigmas: 1.0,
            target_background: 0.25,
        },
        color: ColorMode::PerChannel,
    }
    .apply(&mut unlinked)
    .unwrap();

    let lr = channel(&linked, 0).to_vec();
    let lg = channel(&linked, 1).to_vec();
    let ur = channel(&unlinked, 0).to_vec();
    let ug = channel(&unlinked, 1).to_vec();
    // Color-preserving keeps the red bias in the background.
    assert!(
        lr[0] > lg[0] + 0.1,
        "color-preserving keeps red > green ({}, {})",
        lr[0],
        lg[0]
    );
    // Per-channel pushes each background to the same target -> neutral gray.
    assert!(
        (ur[0] - ug[0]).abs() < 0.05,
        "per-channel neutralizes background ({}, {})",
        ur[0],
        ug[0]
    );
}

// ---- end-to-end ----

#[test]
fn end_to_end_gray_auto_stf_brightens_background_to_target() {
    // Background ~0.05 with spread (MAD > 0) plus bright stars.
    let mut px = Vec::new();
    for i in 0..90 {
        px.push(0.04 + (i % 3) as f32 * 0.01); // {0.04, 0.05, 0.06} -> median 0.05, MAD 0.01
    }
    px.extend(std::iter::repeat_n(0.6f32, 10));
    let input_median = median_of(&px);

    let mut img = gray(10, 10, px);
    Stretch::auto_stf().apply(&mut img).unwrap();
    let out = channel(&img, 0).to_vec();

    for &v in &out {
        assert!((0.0..=1.0).contains(&v), "output out of [0,1]: {v}");
    }
    let out_median = median_of(&out);
    assert!(
        out_median > input_median,
        "background brightened ({out_median} > {input_median})"
    );
    assert!(
        (out_median - 0.25).abs() < 0.05,
        "background lands near target 0.25, got {out_median}"
    );
    // Monotonic mapping: a star pixel (input 0.6) stays brighter than the background.
    assert!(out[95] > out[0], "stars stay brighter than the background");
}

#[test]
fn default_config_is_color_preserving_auto_asinh() {
    let cfg = Stretch::default();
    assert_eq!(cfg.color, ColorMode::ColorPreserving);
    assert!(matches!(cfg.method, StretchMethod::AutoAsinh { .. }));
}

#[test]
fn rejects_out_of_range_config() {
    let mut img = gray(4, 4, vec![0.3; 16]);
    let err = Stretch::ghs(-1.0, 0.0, 0.5).apply(&mut img).unwrap_err();
    assert!(
        matches!(&err, OpError::InvalidConfig(m) if m.contains("ghs d must be")),
        "expected an InvalidConfig ghs error, got {err:?}"
    );
}
