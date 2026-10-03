#[cfg(feature = "real-data")]
mod real_data;

use crate::image_ops::background_extraction::*;
use crate::image_ops::stretching::Stretch;
use crate::internals::images::rgb_image as rgb;
use crate::internals::prelude::*;
use crate::math::statistics::median_mut;

fn fill(size: Size2us, f: impl Fn(usize, usize) -> f32) -> Vec<f32> {
    let mut v = vec![0.0f32; size.pixel_count()];
    for y in 0..size.height {
        for x in 0..size.width {
            v[size.index_of(Vec2us::new(x, y))] = f(x, y);
        }
    }
    v
}

fn gray(size: Size2us, f: impl Fn(usize, usize) -> f32) -> LinearImage {
    gray_image(size, fill(size, f))
}

fn mean(p: &[f32]) -> f64 {
    p.iter().map(|&v| f64::from(v)).sum::<f64>() / p.len() as f64
}

/// Residual energy about the plane's mean — the kept sky level is not residual.
fn energy(p: &[f32]) -> f64 {
    let m = mean(p);
    p.iter().map(|&v| (f64::from(v) - m).powi(2)).sum()
}

#[test]
fn poly_terms_are_correct() {
    assert_eq!(poly_terms(0), vec![(0, 0)]);
    assert_eq!(poly_terms(1), vec![(0, 0), (0, 1), (1, 0)]);
    // total 2 adds (0,2),(1,1),(2,0) → 6 terms = (2+1)(2+2)/2.
    assert_eq!(poly_terms(2).len(), 6);
    assert_eq!(poly_terms(4).len(), 15);
}

#[test]
fn effective_degree_fits_within_samples() {
    // term counts: d1=3, d2=6, d3=10. Need terms ≤ n.
    assert_eq!(effective_degree(10, 3), 3);
    assert_eq!(effective_degree(6, 3), 2); // 10 > 6, 6 ≤ 6
    assert_eq!(effective_degree(3, 3), 1); // 10,6 > 3, 3 ≤ 3
    assert_eq!(effective_degree(2, 3), 0); // even 3 > 2
    assert_eq!(effective_degree(100, 4), 4); // capped at 4
}

/// `0.5 + x/1024 + y/2048`: every value, every symmetric pair's sum and the tile sums are exact
/// in f32 and f64, so each tile's sky — the Pearson mode of point-symmetric samples, see
/// `background_mesh` — is the plane at the tile centre exactly. The least-squares fit then
/// reproduces the plane to a few f64 ulps, which the cast to f32 rounds back onto the dyadic value:
/// the model equals the image, pixel for pixel, and so does its mean equal the plane's.
fn dyadic_plane(x: usize, y: usize) -> f32 {
    0.5 + x as f32 / 1024.0 + y as f32 / 2048.0
}

/// The plane's mean over `size`: its value at the centre pixel index.
fn dyadic_plane_mean(size: Size2us) -> f32 {
    0.5 + (size.width - 1) as f32 / 2048.0 + (size.height - 1) as f32 / 4096.0
}

/// An additive gradient leaves exactly its mean level: `p − (m − mean)` with `m = p` is the mean
/// for every pixel — 0.5 + 199/2048 + 159/4096 — in each of three channels on its own plane.
#[test]
fn subtract_removes_linear_gradients_per_channel() {
    let size = Size2us::new(200, 160);
    let r = fill(size, dyadic_plane);
    let g = fill(size, |x, y| dyadic_plane(size.width - 1 - x, y));
    let b = fill(size, |x, y| 0.25 + y as f32 / 4096.0 + x as f32 / 8192.0);
    let mut img = rgb(size, r, g, b);
    ExtractBackground {
        degree: 1,
        tile_size: 40,
        ..Default::default()
    }
    .apply(&mut img)
    .unwrap();
    let blue_level = 0.25 + 159.0 / 8192.0 + 199.0 / 16384.0;
    for (c, level) in [
        (0, dyadic_plane_mean(size)),
        (1, dyadic_plane_mean(size)),
        (2, blue_level),
    ] {
        assert!(
            img.channel(c).pixels().iter().all(|&v| v == level),
            "channel {c} is flat at {level}"
        );
    }
}

/// A flat sky has no gradient to remove: `Subtract` leaves it exactly as it was, lone star pixels
/// included — a tile's MAD over one bright pixel is 0, so its sky is the median, 0.3125.
#[test]
fn subtract_leaves_a_flat_sky_and_its_stars_unchanged() {
    let size = Size2us::new(128, 128);
    let stars = [(10, 10), (50, 80), (100, 30), (70, 70), (20, 110)];
    let sky = fill(size, |x, y| {
        if stars.contains(&(x, y)) {
            0.9375
        } else {
            0.3125
        }
    });
    let mut img = gray_image(size, sky.clone());
    ExtractBackground {
        degree: 2,
        tile_size: 32,
        ..Default::default()
    }
    .apply(&mut img)
    .unwrap();
    assert_eq!(img.channel(0).pixels(), sky.as_slice());
}

/// A multiplicative falloff divides out to its mean level: `p / (m / mean)` with `m = p` rounds
/// twice, by half an ulp each, so every pixel is the mean to ε relative.
#[test]
fn divide_corrects_a_planar_falloff() {
    let size = Size2us::new(160, 160);
    let mut img = gray(size, dyadic_plane);
    ExtractBackground {
        degree: 2,
        tile_size: 20,
        mode: BackgroundMode::Divide,
        ..Default::default()
    }
    .apply(&mut img)
    .unwrap();
    let level = dyadic_plane_mean(size);
    for &v in img.channel(0).pixels() {
        assert_close!(v, level, f32::EPSILON);
    }
}

/// Divide never amplifies past `1/divide_floor`. On `x/1024 + 1/64` across 160 columns the model
/// over its mean runs from (1/64)/(79.5/1024 + 1/64) ≈ 0.164 up; a floor of ½ caps every column
/// whose ratio falls below it, and a floor of 0.1 none of them. Where the floor holds, the pixel is
/// divided by it exactly once. The ratio is `(x + 16)/95.5`, at least 2.6e-3 from ½ at every
/// column, far past what its rounding could move.
#[test]
fn divide_floor_caps_the_gain() {
    let size = Size2us::new(160, 32);
    let falloff = |x: usize, _: usize| x as f32 / 1024.0 + 1.0 / 64.0;
    let run = |divide_floor| {
        let mut img = gray(size, falloff);
        ExtractBackground {
            degree: 1,
            tile_size: 16,
            mode: BackgroundMode::Divide,
            divide_floor,
            ..Default::default()
        }
        .apply(&mut img)
        .unwrap();
        img.channel(0).pixels().to_vec()
    };
    let (loose, tight) = (run(0.1), run(0.5));
    let mean = f64::from(falloff(0, 0)) + 79.5 / 1024.0;
    let mut floored = 0;
    for x in 0..size.width {
        let p = falloff(x, 0);
        let ratio = f64::from(p) / mean;
        assert!(ratio > 0.1, "no column reaches the loose floor");
        if ratio < 0.5 {
            assert_eq!(tight[x], p / 0.5, "column {x} held at the floor");
            floored += 1;
        } else {
            assert_eq!(tight[x], loose[x], "column {x} above the floor");
        }
    }
    assert!(floored > 0);
}

/// A model with no positive mean has no level to normalize by, so `Divide` refuses it. The model
/// of `−0.25 + x/1024` over columns 0 to 63 averages `−0.25 + 31.5/1024`. Every channel is fitted
/// before any is changed, so the red channel, which divides cleanly, is left as it was when green
/// fails.
#[test]
fn divide_refuses_a_sky_with_no_positive_mean_and_changes_nothing() {
    let size = Size2us::new(64, 64);
    let negative = fill(size, |x, _| -0.25 + x as f32 / 1024.0);
    let positive = fill(size, |x, _| 0.25 + x as f32 / 1024.0);
    let mut img = rgb(size, positive.clone(), negative.clone(), positive.clone());
    let error = ExtractBackground {
        degree: 1,
        tile_size: 16,
        mode: BackgroundMode::Divide,
        ..Default::default()
    }
    .apply(&mut img)
    .unwrap_err();
    // The tolerance absorbs the least-squares fit's rounding in f64.
    assert!(
        matches!(error, OpError::NonPositiveBackground { mean } if (mean - (-0.25 + 31.5 / 1024.0)).abs() < 1e-9),
        "{error:?}"
    );
    assert_eq!(img.channel(0).pixels(), positive.as_slice());
    assert_eq!(img.channel(1).pixels(), negative.as_slice());
}

/// A bright blob over four whole tiles of a 8 × 6 grid would pull the fit toward it. The residual
/// clip rejects those tiles, and the refit on the rest is the plane again — the sky comes out flat
/// at its mean and the blob keeps its 0.5, exactly. With no refit passes, or a rejection threshold
/// too wide to reject anything, the blob tilts the model and the sky is no longer flat.
#[test]
fn outlier_tiles_are_rejected_from_the_fit() {
    let size = Size2us::new(160, 120);
    let blob = |x: usize, y: usize| (40..80).contains(&x) && (40..80).contains(&y);
    let sky = fill(size, |x, y| {
        dyadic_plane(x, y) + if blob(x, y) { 0.5 } else { 0.0 }
    });
    let run = |config: ExtractBackground| {
        let mut img = gray_image(size, sky.clone());
        config.apply(&mut img).unwrap();
        img.channel(0).pixels().to_vec()
    };
    let config = ExtractBackground {
        degree: 1,
        tile_size: 20,
        ..Default::default()
    };

    let level = dyadic_plane_mean(size);
    let rejected = run(config.clone());
    for y in 0..size.height {
        for x in 0..size.width {
            let expected = if blob(x, y) { level + 0.5 } else { level };
            assert_eq!(
                rejected[size.index_of(Vec2us::new(x, y))],
                expected,
                "({x}, {y})"
            );
        }
    }

    let unrefined = run(ExtractBackground {
        iterations: 0,
        ..config.clone()
    });
    let unrejecting = run(ExtractBackground {
        rejection_sigma: 1e6,
        ..config
    });
    assert_ne!(unrefined, rejected);
    assert_eq!(unrejecting, unrefined);
}

/// The fitted surface is evaluated two ways: `eval`, a sum of `powi` monomials, and
/// `Surface::remove`, Horner over row-collapsed coefficients; `Surface::mean` is a third, from
/// closed-form axis moments. On a non-square frame at every degree they agree: the f64 evaluations
/// to 1e-12 of the coefficients' magnitude (a few dozen f64 roundings), the stored model to that
/// plus its f32 rounding, and the mean to the average of `eval` over every pixel.
#[test]
fn surface_evaluators_agree() {
    let size = Size2us::new(37, 23);
    for degree in 1..=4 {
        let terms = poly_terms(degree);
        let coeffs = DVector::from_fn(terms.len(), |k, _| {
            0.3 - 0.17 * k as f64 + 0.011 * (k * k) as f64
        });
        let magnitude: f64 = coeffs.iter().map(|c| c.abs()).sum();
        let surface = Surface::new(&coeffs, &terms, size);
        let mut model = Buffer2::new_filled(size.width, size.height, 0.0f32);
        surface.remove(&mut model, |_, m| m);

        let mut total = 0.0;
        for y in 0..size.height {
            for x in 0..size.width {
                let expected = eval(
                    &coeffs,
                    &terms,
                    norm(x as f64, size.width),
                    norm(y as f64, size.height),
                );
                total += expected;
                let got = f64::from(model[(x, y)]);
                let bound = 1e-12 * magnitude + f64::from(f32::EPSILON) / 2.0 * expected.abs();
                assert!(
                    (got - expected).abs() <= bound,
                    "degree {degree} at ({x}, {y}): {got} vs {expected}"
                );
            }
        }
        let average = total / size.pixel_count() as f64;
        assert!(
            (surface.mean() - average).abs() <= 1e-12 * magnitude,
            "degree {degree}: mean {} vs {average}",
            surface.mean()
        );
    }
}

#[test]
fn higher_degree_fits_cubic_better() {
    let size = Size2us::new(180, 180);
    let cubic = |x: usize, y: usize| {
        let (nx, ny) = (x as f32 / size.width as f32, y as f32 / size.height as f32);
        0.4 + 0.2 * nx - 0.3 * nx * nx + 0.25 * nx * nx * nx + 0.15 * ny * ny * ny
    };
    let resid_energy = |degree| {
        let mut img = gray(size, cubic);
        ExtractBackground {
            degree,
            tile_size: 20,
            ..Default::default()
        }
        .apply(&mut img)
        .unwrap();
        energy(img.channel(0).pixels())
    };
    let e1 = resid_energy(1);
    let e3 = resid_energy(3);
    // A degree-3 surface captures the cubic's curvature; degree-1 cannot represent it at all.
    assert!(
        e3 < 0.02 * e1,
        "deg-3 leaves far less residual than deg-1: e3 {e3:.3e} vs e1 {e1:.3e}"
    );
    // deg-3 removes the cubic to a tight per-pixel residual (only tile-sampling error remains —
    // ~0.2% RMS over a ~0.15-wide range).
    let rms3 = (e3 / size.pixel_count() as f64).sqrt();
    assert!(
        rms3 < 2e-3,
        "deg-3 essentially removes the cubic: residual RMS {rms3:.2e}"
    );
}

#[test]
fn rank_deficient_sample_grid_is_reported_without_mutating_the_image() {
    let mut img = gray(Size2us::new(8, 64), |x, y| {
        0.2 + 0.01 * x as f32 + 0.001 * y as f32
    });
    let before = img.channel(0).pixels().to_vec();

    let err = ExtractBackground {
        degree: 1,
        tile_size: 8,
        iterations: 0,
        ..Default::default()
    }
    .apply(&mut img)
    .unwrap_err();

    assert!(
        matches!(
            &err,
            OpError::RankDeficient {
                operation: "background surface fit",
                rank: 2,
                required_rank: 3,
            }
        ),
        "expected the one-column sample grid's rank failure, got {err:?}"
    );
    assert_eq!(
        img.channel(0).pixels(),
        before,
        "a failed fit must not partially rewrite its channel"
    );
}

/// The standard chain: remove the gradient, then auto-stretch. Each auto stretch must put the
/// background median on its 0.2 target, from a sky subtracted to ≈0 — the input that sends a
/// stretch whose target depends on the median onto a degenerate branch.
///
/// 129×129 is an odd count, so the median is one pixel and a monotone curve maps it exactly;
/// 1e-4 is the asinh solver's own acceptance, and STF's closed form is tighter.
#[test]
fn auto_stretches_hit_their_target_after_gradient_removal() {
    let size = Size2us::new(129, 129);
    let mut rng = TestRng::new(7);
    let sky: Vec<f32> = fill(size, |x, y| {
        let star = if (x % 31, y % 29) == (15, 14) {
            0.6
        } else {
            0.0
        };
        0.08 + 0.0004 * x as f32 + 0.0003 * y as f32 + star
    })
    .into_iter()
    .map(|v| v + 0.002 * rng.next_gaussian_f32())
    .collect();
    for stretch in [Stretch::auto_asinh(), Stretch::auto_stf()] {
        let mut img = gray_image(size, sky.clone());
        ExtractBackground::default().apply(&mut img).unwrap();
        stretch.apply(&mut img).unwrap();
        let mut px = img.channel(0).pixels().to_vec();
        let median = median_mut(&mut px);
        assert!(
            (median - 0.2).abs() < 1e-4,
            "{stretch:?}: background median {median}, want 0.2"
        );
    }
}
