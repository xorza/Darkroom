use crate::bit_buffer2::BitBuffer2;
use crate::io::image::cfa::CfaType;
use crate::io::image::image_metadata::ImageMetadata;
use crate::io::raw::demosaic::bayer::CfaPattern;
use crate::math::statistics::median_f32_mut;
use crate::stacking::calibration_masters::cosmic_ray::config::{CosmicRayConfig, NoiseEstimation};
use crate::stacking::calibration_masters::cosmic_ray::mono::replace_flagged;
use crate::stacking::calibration_masters::cosmic_ray::*;
use crate::testing::cfa_from_plane;
use crate::testing::prelude::*;
use crate::testing::synthetic::sky_field::{Sky, SkyField};
use crate::testing::synthetic::star_profiles::{StarProfile, SyntheticStar};

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
    let count = reject_cosmic_rays(&mut img, &CosmicRayConfig::default());
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
    // 7 injected; allow modest growth but not runaway over-flagging.
    assert!((7..=20).contains(&count), "unexpected CR count: {count}");
}

#[test]
fn clean_field_few_false_positives() {
    let field = synthetic_field();
    let count = reject_cosmic_rays(
        &mut cfa_from_plane(field.pixels, CfaType::Mono),
        &CosmicRayConfig::default(),
    );
    assert!(count <= 2, "clean field should flag ~0 CRs, got {count}");
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
    );
    let strict = reject_cosmic_rays(
        &mut cfa_from_plane(data, CfaType::Mono),
        &CosmicRayConfig {
            sigclip: 60.0,
            ..Default::default()
        },
    );
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
            full_scale: 4095.0,
        },
    ] {
        let mut img = cfa_from_plane(data.clone(), CfaType::Mono);
        let count = reject_cosmic_rays(
            &mut img,
            &CosmicRayConfig {
                noise,
                ..Default::default()
            },
        );
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
    let mut data = Buffer2::new_filled(size.width, size.height, 0.05f32);
    let mut rng = TestRng::new(11);
    for v in data.iter_mut() {
        *v += rng.next_gaussian_f32() * 0.003;
    }
    SyntheticStar::new(
        Vec2::new(24.0, 24.0),
        0.6,
        StarProfile::Gaussian { sigma: 2.5 },
    )
    .add_to(&mut data);
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
    let mut img = CfaImage {
        data,
        metadata: ImageMetadata {
            cfa_type: Some(CfaType::Bayer(CfaPattern::Rggb)),
            ..Default::default()
        },
        quantization_sigma: None,
        nulls: None,
    };
    let count = reject_cosmic_rays(&mut img, &CosmicRayConfig::default());
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
    assert!((4..=24).contains(&count), "unexpected CR count: {count}");
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
    let mut data = Buffer2::new_filled(size.width, size.height, 0.05f32);
    let mut rng = TestRng::new(13);
    for v in data.iter_mut() {
        *v += rng.next_gaussian_f32() * 0.003;
    }
    // σ=1.0 → FWHM≈2.35 px in the mosaic
    SyntheticStar::new(
        Vec2::new(24.0, 24.0),
        0.6,
        StarProfile::Gaussian { sigma: 1.0 },
    )
    .add_to(&mut data);
    let core = Vec2us::new(24, 24);
    let crs = [Vec2us::new(8, 8), Vec2us::new(37, 37)];
    for &p in &crs {
        data[size.index_of(p)] = 0.95;
    }
    let mut img = CfaImage {
        data,
        metadata: ImageMetadata {
            cfa_type: Some(CfaType::Bayer(CfaPattern::Rggb)),
            ..Default::default()
        },
        quantization_sigma: None,
        nulls: None,
    };
    reject_cosmic_rays(&mut img, &CosmicRayConfig::default());
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
    let pattern = [
        [1, 0, 1, 1, 2, 1],
        [2, 1, 2, 0, 1, 0],
        [1, 2, 1, 1, 0, 1],
        [1, 2, 1, 1, 0, 1],
        [0, 1, 0, 2, 1, 2],
        [1, 0, 1, 1, 2, 1],
    ];
    let cfa = CfaType::XTrans(pattern);
    let size = Size2us::new(18, 18);
    let color_val = |c: u8| match c {
        0 => 0.10, // R
        1 => 0.20, // G
        _ => 0.30, // B
    };
    let mut data = vec![0.0f32; size.pixel_count()];
    let mut rng = TestRng::new(5);
    for y in 0..size.height {
        for x in 0..size.width {
            let p = Vec2us::new(x, y);
            data[size.index_of(p)] = color_val(cfa.color_at(p)) + rng.next_gaussian_f32() * 0.002;
        }
    }
    let cr = Vec2us::new(9, 9);
    data[size.index_of(cr)] = 0.95;

    let mut img = CfaImage {
        data: Buffer2::new(size.width, size.height, data),
        metadata: ImageMetadata {
            cfa_type: Some(cfa.clone()),
            ..Default::default()
        },
        quantization_sigma: None,
        nulls: None,
    };
    let count = reject_cosmic_rays(&mut img, &CosmicRayConfig::default());
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
                out[target] = median_f32_mut(&mut buf);
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
