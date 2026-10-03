#![expect(
    clippy::cast_sign_loss,
    reason = "test fixtures are small images, with non-negative coordinates and offsets of a few dozen pixels"
)]

use crate::calibration_masters::cosmic_ray::config::NoiseEstimation;
use crate::calibration_masters::cosmic_ray::mono::replace_flagged;
use crate::calibration_masters::cosmic_ray::*;
use crate::internals::cfa::XTRANS_PATTERN;
use crate::internals::cfa::cfa_from_plane;
use crate::internals::prelude::*;
use crate::internals::synthetic::patterns;
use crate::internals::synthetic::sky_field::{Sky, SkyField};
use crate::io::image::cfa::QUANTIZATION_SIGMA_PER_STEP;
use crate::io::image::pixel_flags::Flags;
use crate::io::raw::demosaic::bayer::CfaPattern;
use crate::math::statistics::median_mut;

/// 64×64: flat sky + deterministic Gaussian noise (σ≈0.003) + three well-sampled stars
/// (FWHM≈3 px). Unclamped, because the tests inject cosmic rays above the ceiling afterwards.
fn synthetic_field() -> SkyField {
    let sky = Sky {
        level: 0.05,
        noise: 0.003,
        clamp: false,
    };
    let stars = [
        (Vec2::new(20.0, 20.0), 0.6),
        (Vec2::new(44.0, 30.0), 0.45),
        (Vec2::new(32.0, 50.0), 0.7),
    ];
    SkyField::render(Size2us::new(64, 64), sky, 1.3, &stars, 7)
}

#[test]
fn removes_cosmic_rays_preserves_stars() {
    let SkyField {
        pixels: mut data,
        centers: star_cores,
    } = synthetic_field();
    let size = Size2us::new(data.width(), data.height());
    // Single-pixel CRs at empty positions + a short horizontal streak.
    let crs = [
        Vec2us::new(10, 10),
        Vec2us::new(54, 12),
        Vec2us::new(12, 54),
        Vec2us::new(50, 50),
    ];
    let streak = [Vec2us::new(30, 8), Vec2us::new(31, 8), Vec2us::new(32, 8)];
    for &p in crs.iter().chain(&streak) {
        data[size.index_of(p)] = 0.95;
    }
    let star_vals: Vec<f32> = star_cores.iter().map(|&p| data[size.index_of(p)]).collect();

    let mut img = cfa_from_plane(data, CfaType::Mono);
    let count = reject_cosmic_rays(&mut img, &CosmicRayConfig::default()).unwrap();
    let out = img.data.pixels();

    // Every injected CR is removed (the spike drops back toward sky, ≪ 0.95).
    for &p in crs.iter().chain(&streak) {
        assert!(
            out[size.index_of(p)] < 0.2,
            "CR at ({},{}) not removed: {}",
            p.x,
            p.y,
            out[size.index_of(p)]
        );
    }
    // Star cores are untouched — the fine-structure test must not flag PSF-broadened peaks.
    for (&p, &orig) in star_cores.iter().zip(&star_vals) {
        assert!(
            (out[size.index_of(p)] - orig).abs() < 1e-6,
            "star core at ({},{}) was altered: {} vs {orig}",
            p.x,
            p.y,
            out[size.index_of(p)]
        );
    }
    // The 7 injected, and (13, 54): it shares the 2×2 subsample block of the CR at (12, 54), which
    // lifts its significance past the growth threshold `sigfrac · sigclip`.
    assert_eq!(count, 8);
}

#[test]
fn clean_field_few_false_positives() {
    let field = synthetic_field();
    let count = reject_cosmic_rays(
        &mut cfa_from_plane(field.pixels, CfaType::Mono),
        &CosmicRayConfig::default(),
    )
    .unwrap();
    assert_eq!(count, 0, "a clean field flags nothing");
}

#[test]
fn sigclip_controls_sensitivity() {
    // A modest spike (~15σ): a sensitive sigclip flags it, a strict one doesn't (A→X, B→Y, X≠Y).
    let SkyField {
        pixels: mut data, ..
    } = synthetic_field();
    let size = Size2us::new(data.width(), data.height());
    data[size.index_of(Vec2us::new(40, 40))] = 0.05 + 0.045;
    let sensitive = reject_cosmic_rays(
        &mut cfa_from_plane(data.clone(), CfaType::Mono),
        &CosmicRayConfig {
            sigclip: 3.0,
            ..Default::default()
        },
    )
    .unwrap();
    let strict = reject_cosmic_rays(
        &mut cfa_from_plane(data, CfaType::Mono),
        &CosmicRayConfig {
            sigclip: 60.0,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        sensitive > strict,
        "lower sigclip must flag more: sensitive={sensitive}, strict={strict}"
    );
}

#[test]
fn empirical_and_parametric_both_catch_a_bright_cr() {
    // Both noise models must flag an obvious bright CR among the stars.
    let SkyField {
        pixels: mut data, ..
    } = synthetic_field();
    let size = Size2us::new(data.width(), data.height());
    let cr = Vec2us::new(15, 33);
    data[size.index_of(cr)] = 0.99;
    for noise in [
        NoiseEstimation::Empirical,
        NoiseEstimation::Parametric {
            gain: 1.5,
            read_noise: 5.0,
        },
    ] {
        let mut img = cfa_from_plane(data.clone(), CfaType::Mono);
        // A 12-bit ADC: one step is 1/4095 of a sample unit.
        img.metadata.quantization_sigma = Some(QUANTIZATION_SIGMA_PER_STEP / 4095.0);
        let count = reject_cosmic_rays(
            &mut img,
            &CosmicRayConfig {
                noise,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(count >= 1, "bright CR missed");
        assert!(
            img.data.pixels()[size.index_of(cr)] < 0.2,
            "bright CR not in-painted"
        );
    }
}

#[test]
fn bayer_removes_cosmic_rays_preserves_star() {
    // Bayer deinterleave path: a well-sampled star + CRs spread across all four 2×2 phases. The
    // CRs go; the star core survives (each phase plane's mono detector protects it).
    let size = Size2us::new(48, 48);
    let sky = Sky {
        level: 0.05,
        noise: 0.003,
        clamp: false,
    };
    let SkyField {
        pixels: mut data, ..
    } = SkyField::render(size, sky, 2.5, &[(Vec2::new(24.0, 24.0), 0.6)], 11);
    let core = Vec2us::new(24, 24);
    let star = data[size.index_of(core)];
    // Each CR sits in a different (x%2, y%2) phase, exercising all four planes.
    let crs = [
        Vec2us::new(8, 8),
        Vec2us::new(9, 12),
        Vec2us::new(12, 41),
        Vec2us::new(37, 37),
    ];
    for &p in &crs {
        data[size.index_of(p)] = 0.95;
    }
    let mut img = cfa_from_plane(data, CfaType::Bayer(CfaPattern::Rggb));
    let count = reject_cosmic_rays(&mut img, &CosmicRayConfig::default()).unwrap();
    let out = img.data.pixels();
    for &p in &crs {
        assert!(
            out[size.index_of(p)] < 0.2,
            "Bayer CR ({},{}) not removed: {}",
            p.x,
            p.y,
            out[size.index_of(p)]
        );
    }
    assert!(
        out[size.index_of(core)] > 0.5,
        "star core gutted: {} (was {star})",
        out[size.index_of(core)]
    );
    // The four CRs, and two pixels the growth pass takes: (11, 10) beside (9, 12) and (35, 39)
    // beside (37, 37), each a diagonal neighbour in its CR's phase plane, inside the 2×2
    // subsample block that lifts its significance past `sigfrac · sigclip`.
    assert_eq!(count, 6);
    // Every in-painted photosite is flagged at its mosaic position, and nothing else is: the four
    // phase planes map back without crossing.
    let flags = img.flags.as_ref().unwrap();
    let repaired = Flags::COSMIC_RAY.union(Flags::REPAIRED);
    assert_eq!(flags.count(Flags::COSMIC_RAY), 6);
    assert_eq!(flags.count(Flags::REPAIRED), 6);
    for &p in crs
        .iter()
        .chain(&[Vec2us::new(11, 10), Vec2us::new(35, 39)])
    {
        assert_eq!(flags.at_pos(p), repaired, "({}, {})", p.x, p.y);
    }
}

#[test]
fn bayer_tight_star_eaten_is_a_known_limitation() {
    // CR-2 characterization (not a goal — a documented limitation). A *tight* star (FWHM≈2.35 px
    // in the mosaic → ~1.2 px in each half-res Bayer phase plane) is undersampled there: median₃
    // erases its core exactly like a cosmic ray, so the fine-structure test F→0 and L.A.Cosmic
    // *cannot* tell the core from a CR at any `objlim`. The per-phase detector therefore eats it.
    // This is why the module doc steers tight / dithered OSC data to stack-time σ-rejection
    // instead of per-frame CFA L.A.Cosmic. The test pins the behavior so a future fix
    // (e.g. mosaic-level detection) flips it loudly. Contrast `bayer_removes_cosmic_rays_...`,
    // which uses a *well-sampled* FWHM≈5.9 px star that survives.
    let size = Size2us::new(48, 48);
    let sky = Sky {
        level: 0.05,
        noise: 0.003,
        clamp: false,
    };
    // σ = 1.0 → FWHM ≈ 2.35 px in the mosaic.
    let SkyField {
        pixels: mut data, ..
    } = SkyField::render(size, sky, 1.0, &[(Vec2::new(24.0, 24.0), 0.6)], 13);
    let core = Vec2us::new(24, 24);
    let crs = [Vec2us::new(8, 8), Vec2us::new(37, 37)];
    for &p in &crs {
        data[size.index_of(p)] = 0.95;
    }
    let mut img = cfa_from_plane(data, CfaType::Bayer(CfaPattern::Rggb));
    reject_cosmic_rays(&mut img, &CosmicRayConfig::default()).unwrap();
    let out = img.data.pixels();
    // CR rejection still works — the injected CRs are removed.
    for &p in &crs {
        assert!(
            out[size.index_of(p)] < 0.2,
            "Bayer CR ({},{}) not removed: {}",
            p.x,
            p.y,
            out[size.index_of(p)]
        );
    }
    // Known limitation: the tight star core is also gutted (flagged as a CR).
    assert!(
        out[size.index_of(core)] < 0.2,
        "tight star core unexpectedly survived ({}) — if per-phase Bayer CR detection was \
         fixed, update this characterization test",
        out[size.index_of(core)]
    );
}

#[test]
fn xtrans_removes_cosmic_ray_preserves_flat_field() {
    // X-Trans same-color path: per-color baselines + tiny noise + one bright CR. The CR is
    // replaced from same-color neighbors (≈ its color's baseline); flat pixels stay put.
    let cfa = CfaType::XTrans(XTRANS_PATTERN);
    let size = Size2us::new(18, 18);
    let color_val = |c: u8| match c {
        0 => 0.10, // R
        1 => 0.20, // G
        _ => 0.30, // B
    };
    let mut data = vec![0.0f32; size.pixel_count()];
    for y in 0..size.height {
        for x in 0..size.width {
            let p = Vec2us::new(x, y);
            data[size.index_of(p)] = color_val(cfa.color_at(p));
        }
    }
    patterns::add_gaussian_noise(&mut data, 0.002, 5);
    let cr = Vec2us::new(9, 9);
    data[size.index_of(cr)] = 0.95;

    let mut img = cfa_from_plane(Buffer2::new(size.width, size.height, data), cfa);
    let count = reject_cosmic_rays(&mut img, &CosmicRayConfig::default()).unwrap();
    let out = img.data.pixels();

    assert!(count >= 1, "X-Trans CR missed");
    // Replaced with the same-color (G, here) neighborhood median, ≈ 0.20 — well below the spike.
    let cr_color = cfa.color_at(cr);
    assert!(
        (out[size.index_of(cr)] - color_val(cr_color)).abs() < 0.05,
        "X-Trans CR not repaired to its color baseline: {}",
        out[size.index_of(cr)]
    );
    // A flat pixel of each color far from the CR is untouched (no false positives).
    for &p in &[Vec2us::new(3, 3), Vec2us::new(4, 3), Vec2us::new(3, 4)] {
        let c = cfa.color_at(p);
        assert!(
            (out[size.index_of(p)] - color_val(c)).abs() < 0.02,
            "flat {c}-pixel ({},{}) altered: {}",
            p.x,
            p.y,
            out[size.index_of(p)]
        );
    }
}

/// An independently written reference for [`replace_flagged`], asserting the property that makes
/// the pass order-independent: it writes only masked pixels and reads only unmasked ones, so no
/// replacement can observe another. Any future attempt to drop the frame copy depends on exactly
/// that, and this is what would catch a change that broke it.
fn replace_flagged_via_snapshot(pixels: &[f32], size: Size2us, mask: &BitBuffer2) -> Vec<f32> {
    let src = pixels.to_vec();
    let mut out = pixels.to_vec();
    let (wi, hi) = (size.width as isize, size.height as isize);
    for y in 0..size.height {
        for x in 0..size.width {
            let target = size.index_of(Vec2us::new(x, y));
            if !mask.get(target) {
                continue;
            }
            let mut buf = Vec::new();
            for dy in -2..=2 {
                let yy = (y as isize + dy).clamp(0, hi - 1) as usize;
                for dx in -2..=2 {
                    let xx = (x as isize + dx).clamp(0, wi - 1) as usize;
                    let j = size.index_of(Vec2us::new(xx, yy));
                    if !mask.get(j) {
                        buf.push(src[j]);
                    }
                }
            }
            if !buf.is_empty() {
                out[target] = median_mut(&mut buf);
            }
        }
    }
    out
}

#[test]
fn replace_flagged_matches_a_snapshot_reference() {
    let size = Size2us::new(12, 12);
    // Distinct values so a median that picked up a replaced neighbour would shift visibly.
    let pixels: Vec<f32> = (0..size.pixel_count()).map(|i| i as f32).collect();

    let mut mask = BitBuffer2::new_default(size);
    let at = |x, y| size.index_of(Vec2us::new(x, y));
    // Isolated hit.
    mask.set(at(6, 6), true);
    // Adjacent 2x2 block: the case that would expose an order-dependent read, since each of the
    // four sits inside the others' 5x5 windows.
    for (x, y) in [(2, 9), (3, 9), (2, 10), (3, 10)] {
        mask.set(at(x, y), true);
    }
    // Corner, to exercise the clamped window.
    mask.set(at(0, 0), true);
    // A hit whose entire 5x5 is masked — nothing to repair from, so it must be left alone.
    for y in 0..5 {
        for x in 7..12 {
            mask.set(at(x, y), true);
        }
    }

    let want = replace_flagged_via_snapshot(&pixels, size, &mask);

    let mut got = pixels.clone();
    replace_flagged(&mut got, size, &mask, &mut Vec::new());

    assert_eq!(got, want);
    // The fully-masked interior keeps its original value rather than picking up a neighbour.
    assert_eq!(got[at(9, 2)], pixels[at(9, 2)]);
    // ...while a repairable hit actually moved.
    assert_ne!(got[at(6, 6)], pixels[at(6, 6)]);
}

/// Each field at the edge of what the detector can run with: the default passes, and every row
/// breaks exactly one field and is rejected on it.
#[test]
fn validate_rejects_each_field_out_of_range() {
    assert_eq!(CosmicRayConfig::default().validate(), Ok(()));
    let parametric = |gain, read_noise| NoiseEstimation::Parametric { gain, read_noise };
    for (field, config) in [
        (
            "cosmic-ray sigclip",
            CosmicRayConfig {
                sigclip: 0.0,
                ..Default::default()
            },
        ),
        (
            "cosmic-ray sigclip",
            CosmicRayConfig {
                sigclip: f32::NAN,
                ..Default::default()
            },
        ),
        (
            "cosmic-ray objlim",
            CosmicRayConfig {
                objlim: -1.0,
                ..Default::default()
            },
        ),
        (
            "cosmic-ray sigfrac",
            CosmicRayConfig {
                sigfrac: 0.0,
                ..Default::default()
            },
        ),
        (
            "cosmic-ray sigfrac",
            CosmicRayConfig {
                sigfrac: 1.5,
                ..Default::default()
            },
        ),
        (
            "cosmic-ray niter",
            CosmicRayConfig {
                niter: 0,
                ..Default::default()
            },
        ),
        (
            "cosmic-ray gain",
            CosmicRayConfig {
                noise: parametric(0.0, 5.0),
                ..Default::default()
            },
        ),
        (
            "cosmic-ray read_noise",
            CosmicRayConfig {
                noise: parametric(1.5, -1.0),
                ..Default::default()
            },
        ),
    ] {
        let error = config.validate().unwrap_err();
        assert_eq!(error.field, field, "{config:?}");
    }
    let edge = CosmicRayConfig {
        sigfrac: 1.0,
        niter: 1,
        noise: parametric(1.5, 0.0),
        ..Default::default()
    };
    assert_eq!(edge.validate(), Ok(()));
}

/// The parametric model reads the frame's ADU scale off its quantization σ: a 12-bit step,
/// `(1/√12)/4095` in sample units, gives back 4095 ADU per unit, to the two roundings of the
/// quotient and its inverse. A frame without one cannot use the model; the empirical one needs
/// nothing from the frame.
#[test]
fn the_parametric_model_takes_its_scale_from_the_frame() {
    let estimation = NoiseEstimation::Parametric {
        gain: 1.5,
        read_noise: 5.0,
    };
    let NoiseModel::Parametric { full_scale, .. } =
        NoiseModel::resolve(&estimation, Some(QUANTIZATION_SIGMA_PER_STEP / 4095.0)).unwrap()
    else {
        panic!("a parametric estimation resolves to the parametric model")
    };
    assert_close!(full_scale, 4095.0, 2.0 * f32::EPSILON * 4095.0);
    assert_eq!(NoiseModel::resolve(&estimation, None), Err(UnknownAdcStep));
    assert_eq!(
        NoiseModel::resolve(&NoiseEstimation::Empirical, None),
        Ok(NoiseModel::Empirical)
    );

    let mut image = cfa_from_plane(synthetic_field().pixels, CfaType::Mono);
    let config = CosmicRayConfig {
        noise: estimation,
        ..Default::default()
    };
    assert_eq!(reject_cosmic_rays(&mut image, &config), Err(UnknownAdcStep));
}
