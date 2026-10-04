use super::*;
use crate::math::pixel_gaussian::PixelGaussian;
use crate::star_detection::centroid::measure_grid::AnnulusRadii;
use crate::star_detection::centroid::stamp::{StampFit, sigma_from_moments};

/// The σ seed is the stamp's second moment about the centre, `σ² = Σw·r² / Σw / 2`: on a centred
/// Gaussian truncated to the stamp's ±10 px it is S₂ / S with S = Σᵢ m(i), S₂ = Σᵢ i²·m(i), `m(i)`
/// the profile's mean over pixel `i` — the box adds its 1/12, the truncation under-reports σ, the
/// more the wider the star — and a wider star seeds wider.
///
/// Each f32 sample is off by at most ε of its value ≤ 1.1 (the profile, then the add onto the
/// sky); over 441 samples with |r²/2 − σ²| ≤ 100 that moves σ² by ≤ 441 · ε · 100 / Σw, and σ by
/// half that over σ.
#[test]
fn sigma_seed_is_the_truncated_second_moment() {
    const SKY: f32 = 0.1;
    let radius = 10;
    for sigma in [1.5f32, 2.0, 2.5, 3.0, 4.0] {
        let pixels = SyntheticStar::new(Vec2::splat(10.0), 1.0, StarProfile::Gaussian { sigma })
            .stamp(Size2us::new(21, 21), SKY);
        let fit = StampFit::prepare::<6>(
            &pixels,
            DVec2::splat(10.0),
            &StampGrid::new(radius),
            SKY,
            None,
        )
        .expect("21x21 stamp at its centre");

        let gaussian = PixelGaussian {
            sigma: f64::from(sigma),
        };
        let g = |i: i32| gaussian.mean_at(f64::from(i));
        let arms = -(radius as i32)..=radius as i32;
        let sum: f64 = arms.clone().map(g).sum();
        let second: f64 = arms.map(|i| f64::from(i * i) * g(i)).sum();
        let expected = (second / sum).sqrt();
        let rounding = 441.0 * f64::from(f32::EPSILON) * 100.0 / sum.powi(2) / (2.0 * expected);
        let seed = f64::from(fit.sigma_est);
        assert!(
            (seed - expected).abs() <= rounding,
            "σ {sigma}: seed {seed}, expected {expected} ± {rounding}"
        );
        assert!(
            expected < (f64::from(sigma).powi(2) + 1.0 / 12.0).sqrt(),
            "σ {sigma}: truncation under-reports the sampled width"
        );
    }
}

/// `StampFit::prepare` walks the stamp once and fills every field, so its extraction is pinned
/// through the constructor.
fn extract(pixels: &Buffer2<f32>, pos: DVec2, radius: usize) -> Option<StampFit> {
    StampFit::prepare::<6>(pixels, pos, &StampGrid::new(radius), 0.0, None)
}

#[test]
fn extract_stamp_valid_center() {
    let pixels = Buffer2::new_filled(64, 64, 0.5f32);

    let fit = extract(&pixels, DVec2::splat(32.0), 5).expect("stamp at centre");
    assert_eq!(fit.stamp.z.len(), 11 * 11);
    // Coordinates live in the shared `StampGrid`; what the stamp itself pins is where its
    // top-left pixel sits, which is what the grid's `0..2r` is relative to.
    assert_eq!(fit.stamp.origin, DVec2::new(27.0, 27.0));
    assert_eq!(fit.stamp.peak, 0.5);
    // A flat stamp weights every pixel equally, so `local_pos` lands on the stamp's own centre.
    assert_eq!(fit.local_pos, DVec2::splat(5.0));
    assert!(
        fit.weights.is_none(),
        "unweighted unless a noise model is set"
    );
}

#[test]
fn extract_stamp_edge_invalid() {
    let pixels = Buffer2::new_filled(64, 64, 0.5f32);

    assert!(extract(&pixels, DVec2::new(3.0, 32.0), 5).is_none());
    assert!(extract(&pixels, DVec2::new(32.0, 3.0), 5).is_none());
    assert!(extract(&pixels, DVec2::new(61.0, 32.0), 5).is_none());
    assert!(extract(&pixels, DVec2::new(32.0, 61.0), 5).is_none());
}

#[test]
fn extract_stamp_peak_value() {
    let mut pixels = Buffer2::new_filled(64, 64, 0.1f32);
    pixels[(32, 32)] = 0.9;

    let fit = extract(&pixels, DVec2::splat(32.0), 5).expect("stamp at centre");
    assert_eq!(fit.stamp.peak, 0.9);
}

#[test]
fn extract_stamp_coordinates() {
    let pixels = Buffer2::new_filled(64, 64, 0.5f32);

    // A radius-2 stamp about (32, 32) spans x, y 30..=34: its origin at (30, 30) plus the grid's
    // own 0..=4.
    let fit = extract(&pixels, DVec2::splat(32.0), 2).expect("stamp at centre");
    assert_eq!(fit.stamp.z.len(), 25);
    assert_eq!(fit.stamp.origin, DVec2::new(30.0, 30.0));
}

#[test]
fn extract_stamp_fractional_position() {
    let pixels = Buffer2::new_filled(64, 64, 0.5f32);

    // Fractional position 32.3, 32.7 rounds to 32, 33, so the top-left pixel is (30, 31) and the
    // centre sits at (2.3, 1.7) within the stamp.
    let fit = extract(&pixels, DVec2::new(32.3, 32.7), 2).expect("stamp at centre");
    assert_eq!(fit.stamp.origin, DVec2::new(30.0, 31.0));
    assert!((fit.local_pos - DVec2::new(2.3, 1.7)).length() < 1e-12);
}

#[test]
fn stamp_too_small_for_the_parameter_count_is_rejected() {
    let pixels = Buffer2::new_filled(64, 64, 0.5f32);
    let grid = StampGrid::new(1);
    // A least-squares fit needs strictly more samples than parameters: a radius-1 stamp holds 9 >
    // 6, a radius-0 stamp 1.
    assert!(StampFit::prepare::<6>(&pixels, DVec2::splat(32.0), &grid, 0.0, None).is_some());
    let point = StampGrid::new(0);
    assert!(StampFit::prepare::<6>(&pixels, DVec2::splat(32.0), &point, 0.0, None).is_none());
}

/// A sky the map left in the residual — a flat pedestal Δ — stays in the flux under `GlobalMap` and
/// comes out under `LocalAnnulus`.
///
/// The star is σ = 1.5 on a matched stamp, r = 7. Its annulus runs from 15 to 22 px, past 99% of a
/// Moffat's flux and far past the Gaussian's, so its median is Δ to the f32 rounding and the
/// annulus flux the true flux to within that over npix. The pedestal adds exactly npix · Δ to the
/// map's flux, to the f32 rounding of each sample (≤ ε of A + Δ) and of the sum.
///
/// A flat pedestal barely moves the converged centre: at the fixed point the window is symmetric
/// about it, so the pedestal's windowed offset vanishes there, to the sampling of the window
/// (measured 6.4e-7 px, held to 2e-6). With the annulus taking Δ out, the centre is the pedestal
/// free one to the f32 rounding (measured 2.9e-8 px, held to 1e-7).
#[test]
fn local_annulus_removes_a_sky_the_map_left() {
    const AMPLITUDE: f32 = 0.8;
    const PEDESTAL: f32 = 0.02;
    let sigma = 1.5f32;
    let fwhm = sigma_to_fwhm(sigma);
    let radius = MeasureGrid::stamp_radius(fwhm);
    assert_eq!(radius, 7);
    let pixels = SyntheticStar::new(
        Vec2::new(64.3, 63.6),
        AMPLITUDE,
        StarProfile::Gaussian { sigma },
    )
    .stamp(Size2us::new(128, 128), 0.1);
    let truth = Measured::flat(&pixels, 0.1, 0.01);
    let offset = Measured::flat(&pixels, 0.1 - PEDESTAL, 0.01);
    let region = truth.region_at(DVec2::new(64.3, 63.6));
    let measure = |measured: &Measured, local_background| {
        let config = MeasurementConfig {
            local_background,
            ..Default::default()
        };
        measured
            .measure(&region, &config, fwhm)
            .expect("the star measures")
    };
    let exact = measure(&truth, LocalBackgroundMethod::GlobalMap);
    let global = measure(&offset, LocalBackgroundMethod::GlobalMap);
    let annulus = measure(&offset, LocalBackgroundMethod::LocalAnnulus);

    assert!((global.pos - exact.pos).length() <= 2e-6, "{}", global.pos);
    assert!(
        (annulus.pos - exact.pos).length() <= 1e-7,
        "{}",
        annulus.pos
    );
    let npix = ((2 * radius + 1) * (2 * radius + 1)) as f64;
    let flux = f64::from(exact.flux);
    let rounding = npix * f64::from(f32::EPSILON) * f64::from(AMPLITUDE + PEDESTAL) * 2.0;
    let added = f64::from(global.flux) - flux;
    assert!(
        (added - npix * f64::from(PEDESTAL)).abs() <= rounding,
        "the map keeps {added}, expected {}",
        npix * f64::from(PEDESTAL)
    );
    let removed = f64::from(annulus.flux) - flux;
    assert!(
        removed.abs() <= rounding,
        "the annulus leaves {removed} of the pedestal, bound {rounding}"
    );
}

/// The annulus is `None` below 10 in-frame pixels. A ring of r² ∈ [1, 4] holds 12: (±1, 0),
/// (0, ±1), (±1, ±1), (±2, 0), (0, ±2). About (1, 1), (−2, 0) and (0, −2) fall off the frame, and
/// (2, 0) past a frame 3 wide: 9 are left. A frame 4 wide keeps (2, 0): 10, and the flat
/// residual's median and σ come back exactly.
#[test]
fn local_annulus_needs_ten_pixels_in_the_frame() {
    let annulus = |width| {
        let residual = Buffer2::new_filled(width, 5, 0.25f32);
        LocalBackground::measure(
            &residual,
            None,
            DVec2::splat(1.0),
            AnnulusRadii { inner: 1, outer: 2 },
        )
    };
    assert!(annulus(3).is_none());
    let sky = annulus(4).expect("ten pixels are enough");
    assert_eq!(sky.offset, 0.25);
    assert_eq!(sky.noise, 0.0);
}

/// The annulus measures the sky around a star and not the star: a 9.0 disk inside the inner
/// radius, 17 at FWHM 4, leaves a 0.25 sky exactly, with no spread; an annulus wholly outside the
/// frame has no pixels and falls back to the map. A wide annulus, from 81 at FWHM 20, is
/// subsampled and still reads the flat sky.
#[test]
fn the_annulus_reads_the_sky_around_the_star() {
    let size = Size2us::new(240, 240);
    let centre = DVec2::splat(120.0);
    let mut residual = Buffer2::new_filled(size.width, size.height, 0.25f32);
    for y in 104..137 {
        for x in 104..137 {
            if (x as f64 - 120.0).hypot(y as f64 - 120.0) < 16.0 {
                residual[(x, y)] = 9.0;
            }
        }
    }
    for fwhm in [4.0, 20.0] {
        let grid = MeasureGrid::new(fwhm);
        let sky = LocalBackground::measure(&residual, None, centre, grid.annulus).unwrap();
        assert_eq!((sky.offset, sky.noise), (0.25, 0.0), "FWHM {fwhm}");
    }
    let outside = DVec2::splat(-200.0);
    assert!(
        LocalBackground::measure(&residual, None, outside, MeasureGrid::new(4.0).annulus).is_none()
    );
}

/// The seed must respect the ceiling it is handed, because the optimizer clamps to that same
/// bound on its first iteration — seeding above it just spends an iteration being pulled back.
#[test]
fn sigma_seed_honours_its_ceiling() {
    // The sums a flat 21x21 patch one unit above the sky produces about its centre: every pixel
    // weighs 1, so sum_w = 441, and E[dx²] = E[dy²] = 2·(1²+..+10²)/21 = 770/21, so
    // sum_r2 = 441 · 2 · 770/21 = 32340. That gives sigma = sqrt(32340/441/2) = sqrt(36.667)
    // = 6.0553.
    let sum_w = 441.0;
    let sum_r2 = 32340.0;

    let wide = sigma_from_moments(sum_r2, sum_w, 15.0);
    assert!(
        (wide - 6.0553).abs() < 1e-3,
        "grid's own moment is 6.0553, got {wide}"
    );

    // The tightest ceiling the detector ever uses is MIN_STAMP_RADIUS; the same data has to seed
    // inside it rather than at a fixed 10.0.
    let narrow = sigma_from_moments(sum_r2, sum_w, 4.0);
    assert_eq!(narrow, 4.0);
    assert_ne!(narrow, wide, "the ceiling has to change the answer");

    // No signal above the sky leaves the moment undefined, so the seed falls back rather than
    // dividing by zero.
    assert_eq!(sigma_from_moments(0.0, 0.0, 15.0), 2.0);
}
