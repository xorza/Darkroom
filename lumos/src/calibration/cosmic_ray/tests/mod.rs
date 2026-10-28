#![expect(
    clippy::cast_sign_loss,
    reason = "test fixtures are small images, with non-negative coordinates and offsets of a few dozen pixels"
)]

mod scratch;

use common::{Buffer2, PerWorker};
use glam::{USizeVec2, Vec2};

use crate::bg_mesh::color_mesh::LocalBg;
use crate::calibration::cosmic_ray::cosmic_ray_config::NoiseEstimation;
use crate::calibration::cosmic_ray::masks::{CrMasks, DetectionPlanes};
use crate::calibration::cosmic_ray::mono::internals::median_window;
use crate::calibration::cosmic_ray::mono::{PixelRepair, WindowScratch};
use crate::calibration::cosmic_ray::noise_model::PixelBg;
use crate::calibration::cosmic_ray::*;
use crate::image::flags::pixel_flags::PixelFlags;
use crate::internals::assertions::bits;
use crate::internals::cfa::{XTRANS_PATTERN, cfa_from_plane};
use crate::internals::synthetic::sky_field::{GaussianStars, Sky, SkyField, SkyNoise};
use crate::internals::test_rng::TestRng;
use crate::math::statistics;
use crate::math::statistics::median_mad::MedianMad;
use crate::noise::QUANTIZATION_SIGMA_PER_STEP;

/// 64×64: flat sky + deterministic Gaussian noise (σ≈0.003) + three well-sampled stars
/// (FWHM≈3 px). Unclamped, because the tests inject cosmic rays above the ceiling afterwards.
fn synthetic_field() -> SkyField {
    let sky = Sky {
        level: 0.05,
        noise: Some(SkyNoise {
            sigma: 0.003,
            seed: 7,
        }),
        clamp: false,
    };
    let stars = [
        (Vec2::new(20.0, 20.0), 0.6),
        (Vec2::new(44.0, 30.0), 0.45),
        (Vec2::new(32.0, 50.0), 0.7),
    ];
    SkyField::render(
        USize2::new(64, 64),
        sky,
        GaussianStars {
            sigma: 1.3,
            peaks: &stars,
        },
    )
}

#[test]
fn removes_cosmic_rays_preserves_stars() {
    let SkyField {
        pixels: mut data,
        centers: star_cores,
    } = synthetic_field();
    let size = data.size();
    // Single-pixel CRs at empty positions + a short horizontal streak.
    let crs = [
        USizeVec2::new(10, 10),
        USizeVec2::new(54, 12),
        USizeVec2::new(12, 54),
        USizeVec2::new(50, 50),
    ];
    let streak = [
        USizeVec2::new(30, 8),
        USizeVec2::new(31, 8),
        USizeVec2::new(32, 8),
    ];
    for &p in crs.iter().chain(&streak) {
        data[size.idx_of(p)] = 0.95;
    }
    let star_vals: Vec<f32> = star_cores.iter().map(|&p| data[size.idx_of(p)]).collect();

    let mut img = cfa_from_plane(data, CfaType::Mono);
    let count = reject(&mut img, &CosmicRayConfig::default()).unwrap();
    let out = img.data().pixels();

    // Every injected CR is removed (the spike drops back toward sky, ≪ 0.95).
    for &p in crs.iter().chain(&streak) {
        assert!(
            out[size.idx_of(p)] < 0.2,
            "CR at ({},{}) not removed: {}",
            p.x,
            p.y,
            out[size.idx_of(p)]
        );
    }
    // Star cores are untouched — the fine-structure test must not flag PSF-broadened peaks.
    for (&p, &orig) in star_cores.iter().zip(&star_vals) {
        assert!(
            (out[size.idx_of(p)] - orig).abs() < 1e-6,
            "star core at ({},{}) was altered: {} vs {orig}",
            p.x,
            p.y,
            out[size.idx_of(p)]
        );
    }
    // The 7 injected, and the two pixels the growth takes beside them: (13, 54) beside (12, 54),
    // and (51, 49) diagonal to (50, 50), each inside its hit's box and clearing
    // `growth_fraction · detection_sigma`. The growth tests no contrast, as astroscrappy's does
    // not.
    assert_eq!(count, 9);
}

#[test]
fn clean_field_few_false_positives() {
    let field = synthetic_field();
    let count = reject(
        &mut cfa_from_plane(field.pixels, CfaType::Mono),
        &CosmicRayConfig::default(),
    )
    .unwrap();
    assert_eq!(count, 0, "a clean field flags nothing");
}

#[test]
fn sigclip_controls_sensitivity() {
    // A modest spike (~15σ): a sensitive detection_sigma flags it, a strict one doesn't (A→X, B→Y,
    // X≠Y).
    let SkyField {
        pixels: mut data, ..
    } = synthetic_field();
    let size = data.size();
    data[size.idx_of(USizeVec2::new(40, 40))] = 0.05 + 0.045;
    let sensitive = reject(
        &mut cfa_from_plane(data.clone(), CfaType::Mono),
        &CosmicRayConfig {
            detection_sigma: 3.0,
            ..Default::default()
        },
    )
    .unwrap();
    let strict = reject(
        &mut cfa_from_plane(data, CfaType::Mono),
        &CosmicRayConfig {
            detection_sigma: 60.0,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        sensitive > strict,
        "lower detection_sigma must flag more: sensitive={sensitive}, strict={strict}"
    );
}

#[test]
fn measured_and_stated_gains_both_catch_a_bright_cr() {
    // Both noise models must flag an obvious bright CR among the stars.
    let SkyField {
        pixels: mut data, ..
    } = synthetic_field();
    let size = data.size();
    let cr = USizeVec2::new(15, 33);
    data[size.idx_of(cr)] = 0.99;
    for noise in [
        NoiseEstimation::Measured,
        NoiseEstimation::Gain {
            electrons_per_adu: 1.5,
        },
    ] {
        let mut img = cfa_from_plane(data.clone(), CfaType::Mono);
        // A 12-bit ADC: one step is 1/4095 of a sample unit.
        img.metadata
            .derived
            .set_quantization_sigma(Some(QUANTIZATION_SIGMA_PER_STEP / 4095.0));
        let count = reject(
            &mut img,
            &CosmicRayConfig {
                noise,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(count >= 1, "bright CR missed");
        assert!(
            img.data().pixels()[size.idx_of(cr)] < 0.2,
            "bright CR not in-painted"
        );
    }
}

#[test]
fn bayer_removes_cosmic_rays_preserves_star() {
    // Bayer deinterleave path: a well-sampled star + CRs spread across all four 2×2 phases. The
    // CRs go; the star core survives (each phase plane's mono detector protects it).
    let size = USize2::new(48, 48);
    let sky = Sky {
        level: 0.05,
        noise: Some(SkyNoise {
            sigma: 0.003,
            seed: 11,
        }),
        clamp: false,
    };
    let SkyField {
        pixels: mut data, ..
    } = SkyField::render(
        size,
        sky,
        GaussianStars {
            sigma: 2.5,
            peaks: &[(Vec2::new(24.0, 24.0), 0.6)],
        },
    );
    let core = USizeVec2::new(24, 24);
    let star = data[size.idx_of(core)];
    // Each CR sits in a different (x%2, y%2) phase, exercising all four planes.
    let crs = [
        USizeVec2::new(8, 8),
        USizeVec2::new(9, 12),
        USizeVec2::new(12, 41),
        USizeVec2::new(37, 37),
    ];
    for &p in &crs {
        data[size.idx_of(p)] = 0.95;
    }
    let mut img = cfa_from_plane(data, CfaType::Mosaic(Mosaic::Bayer(BayerPattern::Rggb)));
    let count = reject(&mut img, &CosmicRayConfig::default()).unwrap();
    let out = img.data().pixels();
    for &p in &crs {
        assert!(
            out[size.idx_of(p)] < 0.2,
            "Bayer CR ({},{}) not removed: {}",
            p.x,
            p.y,
            out[size.idx_of(p)]
        );
    }
    assert!(
        out[size.idx_of(core)] > 0.5,
        "star core gutted: {} (was {star})",
        out[size.idx_of(core)]
    );
    // The four CRs, and three pixels the growth pass takes: (10, 10) beside (8, 8), (11, 10) beside
    // (9, 12) and (35, 39) beside (37, 37), each a diagonal neighbour in its CR's phase plane,
    // inside the 2×2 subsample block that lifts its significance past
    // `growth_fraction · detection_sigma`. The noise is the local sky σ of each color, which the
    // star does not inflate as it inflated the whole-plane MAD, so (8, 8) grows as the others do.
    assert_eq!(count, 7);
    // Every in-painted photosite is flagged at its mosaic position, and nothing else is: the four
    // phase planes map back without crossing.
    let flags = img.flags().unwrap();
    let repaired = QualityFlags::COSMIC_RAY.union(QualityFlags::REPAIRED);
    assert_eq!(flags.count(QualityFlags::COSMIC_RAY), 7);
    assert_eq!(flags.count(QualityFlags::REPAIRED), 7);
    for &p in crs.iter().chain(&[
        USizeVec2::new(10, 10),
        USizeVec2::new(11, 10),
        USizeVec2::new(35, 39),
    ]) {
        assert_eq!(flags.at_pos(p), repaired, "({}, {})", p.x, p.y);
    }
}

#[test]
fn bayer_tight_star_eaten_is_a_known_limitation() {
    // CR-2 characterization (not a goal — a documented limitation). A *tight* star (FWHM≈2.35 px in
    // the mosaic → ~1.2 px in each half-res Bayer phase plane) is undersampled there: median₃
    // erases its core exactly like a cosmic ray, so the fine-structure test F→0 and L.A.Cosmic
    // *cannot* tell the core from a CR at any `contrast_limit`. The per-phase detector therefore
    // eats it. This is why the module doc steers tight / dithered OSC data to stack-time
    // σ-rejection instead of per-frame CFA L.A.Cosmic. The test pins the behavior so a future fix
    // (e.g. mosaic-level detection) flips it loudly. Contrast `bayer_removes_cosmic_rays_...`,
    // which uses a *well-sampled* FWHM≈5.9 px star that survives.
    let size = USize2::new(48, 48);
    let sky = Sky {
        level: 0.05,
        noise: Some(SkyNoise {
            sigma: 0.003,
            seed: 13,
        }),
        clamp: false,
    };
    // σ = 1.0 → FWHM ≈ 2.35 px in the mosaic.
    let SkyField {
        pixels: mut data, ..
    } = SkyField::render(
        size,
        sky,
        GaussianStars {
            sigma: 1.0,
            peaks: &[(Vec2::new(24.0, 24.0), 0.6)],
        },
    );
    let core = USizeVec2::new(24, 24);
    let crs = [USizeVec2::new(8, 8), USizeVec2::new(37, 37)];
    for &p in &crs {
        data[size.idx_of(p)] = 0.95;
    }
    let mut img = cfa_from_plane(data, CfaType::Mosaic(Mosaic::Bayer(BayerPattern::Rggb)));
    reject(&mut img, &CosmicRayConfig::default()).unwrap();
    let out = img.data().pixels();
    // CR rejection still works — the injected CRs are removed.
    for &p in &crs {
        assert!(
            out[size.idx_of(p)] < 0.2,
            "Bayer CR ({},{}) not removed: {}",
            p.x,
            p.y,
            out[size.idx_of(p)]
        );
    }
    // Known limitation: the tight star core is also gutted (flagged as a CR).
    assert!(
        out[size.idx_of(core)] < 0.2,
        "tight star core unexpectedly survived ({}) — if per-phase Bayer CR detection was \
         fixed, update this characterization test",
        out[size.idx_of(core)]
    );
}

/// An independently written reference for [`PixelRepair::apply`], asserting the property that makes
/// the pass order-independent: it writes only masked pixels and reads only unmasked ones, so no
/// replacement can observe another. Any future attempt to drop the frame copy depends on exactly
/// that, and this is what would catch a change that broke it. A pixel with no unmasked neighbour
/// within the 5×5 window widens it a ring at a time to 15×15, then takes `sky`.
fn repair_via_snapshot(pixels: &Buffer2<f32>, mask: &BitBuffer2, sky: f32) -> Buffer2<f32> {
    let size = pixels.size();
    let src = pixels;
    let mut out = pixels.clone();
    let (wi, hi) = (size.width as isize, size.height as isize);
    for y in 0..size.height {
        for x in 0..size.width {
            let target = size.idx_of(USizeVec2::new(x, y));
            if !mask.get(target) {
                continue;
            }
            let mut buf = Vec::new();
            for reach in 2..=7isize {
                for dy in -reach..=reach {
                    let yy = (y as isize + dy).clamp(0, hi - 1) as usize;
                    for dx in -reach..=reach {
                        if reach > 2 && dx.abs().max(dy.abs()) != reach {
                            continue;
                        }
                        let xx = (x as isize + dx).clamp(0, wi - 1) as usize;
                        let j = size.idx_of(USizeVec2::new(xx, yy));
                        if !mask.get(j) {
                            buf.push(src[j]);
                        }
                    }
                }
                if !buf.is_empty() {
                    break;
                }
            }
            out[target] = if buf.is_empty() {
                sky
            } else {
                statistics::median_mut(&mut buf)
            };
        }
    }
    out
}

#[test]
fn a_repair_matches_a_snapshot_reference() {
    let size = USize2::new(12, 12);
    // Distinct values so a median that picked up a replaced neighbour would shift visibly.
    let pixels = Buffer2::from_fn((12, 12), |at| (at.y * 12 + at.x) as f32);

    let mut mask = BitBuffer2::new_default(size);
    let at = |x, y| size.idx_of(USizeVec2::new(x, y));
    // Isolated hit.
    mask.set(at(6, 6), true);
    // Adjacent 2x2 block: the case that would expose an order-dependent read, since each of the
    // four sits inside the others' 5x5 windows.
    for (x, y) in [(2, 9), (3, 9), (2, 10), (3, 10)] {
        mask.set(at(x, y), true);
    }
    // Corner, to exercise the clamped window.
    mask.set(at(0, 0), true);
    // A hit whose entire 5x5 is masked: the window widens to the first ring with an unmasked
    // pixel. For (9, 2) that is reach 3, rows y −1 to 5 (−1 clamped to 0) and columns x 6 to 12
    // (12 clamped to 11). Its unmasked pixels are x = 6 of the top row (6) and of rows 0 to 4
    // (6, 18, 30, 42, 54), and the bottom row y = 5 (66 to 71, then 71 again for x = 12): 13
    // values, 6, 6, 18, 30, 42, 54, 66, 67, 68, 69, 70, 71, 71, whose median, the 7th, is 66.
    for y in 0..5 {
        for x in 7..12 {
            mask.set(at(x, y), true);
        }
    }

    let sky = 0.5;
    let want = repair_via_snapshot(&pixels, &mask, sky);
    let local = |_| PixelBg {
        local: LocalBg { sky, noise: 1.0 },
        flat_gain: 1.0,
    };
    let repair = |pixels: &Buffer2<f32>, mask: &BitBuffer2| {
        let mut got = pixels.clone();
        PixelRepair {
            flagged: mask,
            unread: None,
            local: &local,
        }
        .apply(
            &mut got,
            &mut Buffer2::default(),
            &PerWorker::new(WindowScratch::default),
        );
        got
    };

    let got = repair(&pixels, &mask);
    assert_eq!(got, want);
    assert_eq!(got[at(9, 2)], 66.0);
    assert_ne!(got[at(6, 6)], pixels[at(6, 6)]);

    // With every pixel masked no ring holds a measurement, and each takes its local sky.
    let mut everything = BitBuffer2::new_default(size);
    everything.fill(true);
    let got = repair(&pixels, &everything);
    assert!(got.pixels().iter().all(|&value| value == sky), "{got:?}");
}

/// Each field at the edge of what the detector can run with: the default passes, and every row
/// breaks exactly one field and is rejected on it.
#[test]
fn validate_rejects_each_field_out_of_range() {
    assert_eq!(CosmicRayConfig::default().validate(), Ok(()));
    let gain = |electrons_per_adu| NoiseEstimation::Gain { electrons_per_adu };
    for (field, config) in [
        (
            "cosmic-ray detection_sigma",
            CosmicRayConfig {
                detection_sigma: 0.0,
                ..Default::default()
            },
        ),
        (
            "cosmic-ray detection_sigma",
            CosmicRayConfig {
                detection_sigma: f32::NAN,
                ..Default::default()
            },
        ),
        (
            "cosmic-ray contrast_limit",
            CosmicRayConfig {
                contrast_limit: -1.0,
                ..Default::default()
            },
        ),
        (
            "cosmic-ray growth_fraction",
            CosmicRayConfig {
                growth_fraction: 0.0,
                ..Default::default()
            },
        ),
        (
            "cosmic-ray growth_fraction",
            CosmicRayConfig {
                growth_fraction: 1.5,
                ..Default::default()
            },
        ),
        (
            "cosmic-ray max_iterations",
            CosmicRayConfig {
                max_iterations: 0,
                ..Default::default()
            },
        ),
        (
            "cosmic-ray electrons_per_adu",
            CosmicRayConfig {
                noise: gain(0.0),
                ..Default::default()
            },
        ),
        (
            "cosmic-ray electrons_per_adu",
            CosmicRayConfig {
                noise: gain(f32::INFINITY),
                ..Default::default()
            },
        ),
    ] {
        let error = config.validate().unwrap_err();
        assert_eq!(error.field, field, "{config:?}");
    }
    let edge = CosmicRayConfig {
        growth_fraction: 1.0,
        max_iterations: 1,
        noise: gain(1.5),
        ..Default::default()
    };
    assert_eq!(edge.validate(), Ok(()));
}

/// A stated gain on a frame that records no unit for it is refused, and the pass reports why.
#[test]
fn a_stated_gain_needs_a_unit_on_the_frame() {
    let mut image = cfa_from_plane(synthetic_field().pixels, CfaType::Mono);
    let config = CosmicRayConfig {
        noise: NoiseEstimation::Gain {
            electrons_per_adu: 1.5,
        },
        ..Default::default()
    };
    assert_eq!(
        reject(&mut image, &config),
        Err(CosmicRayError::UnknownAdcStep)
    );
}

/// A faint hit on a steep sky gradient is caught: the noise is each pixel's local σ about the
/// mesh's tile planes, which the gradient does not inflate. A 128 × 128 mono frame
/// with the sky rising from 0.05 to 0.45 across it and white noise of σ 0.002; a hit of +0.04,
/// 20σ, at (70, 70). The whole frame's MAD reads the gradient, about 0.1 in σ, and against that the
/// hit is under half a σ: the old whole-frame model would never flag it.
#[test]
fn a_faint_hit_on_a_gradient_is_caught() {
    let size = USize2::new(128, 128);
    let mut rng = TestRng::new(0x6A);
    let mut pixels = Buffer2::from_fn((128, 128), |at| {
        0.05 + 0.4 * at.x as f32 / 128.0 + 0.002 * rng.next_gaussian_f32()
    });
    let hit = USizeVec2::new(70, 70);
    pixels[size.idx_of(hit)] += 0.04;
    let whole_frame_sigma = MedianMad::of_mut(&mut pixels.clone()).sigma();
    assert!(whole_frame_sigma > 0.08, "premise: {whole_frame_sigma}");
    let mut image = cfa_from_plane(pixels, CfaType::Mono);
    reject(&mut image, &CosmicRayConfig::default()).unwrap();
    assert!(
        image
            .flags()
            .unwrap()
            .at_pos(hit)
            .intersects(QualityFlags::COSMIC_RAY),
        "the hit was missed"
    );
}

/// The growth is astroscrappy's: the box about every hit, kept where `S' > detection_sigma`, then
/// the box about that, kept where `S' > detection_sigma·growth_fraction`, with no contrast test on
/// either. On an 8×3 frame with `detection_sigma` 5, `growth_fraction` 0.3 (so 1.5) and
/// `contrast_limit` 5, row 1 holds the hit P at x = 2 (S' 10, no fine structure), A at x = 3 (S' 6,
/// but fine structure 10 σ, so it fails the contrast), B at x = 4 and C at x = 5 (S' 2 each), and E
/// at x = 0 (S' 6, fine structure 10 σ, so no hit of its own); D below P has S' 2. P is the hit. A
/// is in its box and clears `detection_sigma`: the first ring takes it, contrast or not. B is in
/// A's box and clears 1.5, and so is D in P's: the second ring takes both. C is in no first-ring
/// pixel's box, and E in none at all. Four pixels: P, A, B and D. One ring with the contrast took
/// two.
#[test]
fn the_mask_grows_in_astroscrappys_two_rings() {
    let size = USize2::new(8, 3);
    let at = |x: usize, y: usize| y * size.width + x;
    let mut significance = vec![0.0f32; size.pixel_count()];
    let mut fine = vec![0.0f32; size.pixel_count()];
    let noise = vec![1.0f32; size.pixel_count()];
    significance[at(2, 1)] = 10.0;
    significance[at(3, 1)] = 6.0;
    fine[at(3, 1)] = 10.0;
    significance[at(4, 1)] = 2.0;
    significance[at(5, 1)] = 2.0;
    significance[at(0, 1)] = 6.0;
    fine[at(0, 1)] = 10.0;
    significance[at(2, 2)] = 2.0;
    let cfg = CosmicRayConfig {
        detection_sigma: 5.0,
        growth_fraction: 0.3,
        contrast_limit: 5.0,
        ..CosmicRayConfig::default()
    };
    // Sized from a larger plane's detection, as a Bayer mosaic's later phases are: clearing to the
    // plane at hand must leave none of the earlier plane's bits behind.
    let mut masks = CrMasks::empty();
    let larger = USize2::new(16, 4);
    masks.clear_to(larger);
    let crowded = vec![10.0f32; larger.pixel_count()];
    let flat = vec![0.0f32; larger.pixel_count()];
    let unit = vec![1.0f32; larger.pixel_count()];
    let crowded = DetectionPlanes {
        significance: &crowded,
        fine: &flat,
        noise: &unit,
    };
    assert_eq!(
        masks.detect_and_grow(crowded, &cfg, None),
        larger.pixel_count()
    );
    masks.clear_to(size);
    let planes = DetectionPlanes {
        significance: &significance,
        fine: &fine,
        noise: &noise,
    };
    assert_eq!(masks.detect_and_grow(planes, &cfg, None), 4);
    let flagged: Vec<usize> = (0..size.pixel_count())
        .filter(|&idx| masks.accumulated.get(idx))
        .collect();
    assert_eq!(flagged, [at(2, 1), at(3, 1), at(4, 1), at(2, 2)]);
    // A second pass finds nothing new: every pixel it would take is held already.
    assert_eq!(masks.detect_and_grow(planes, &cfg, None), 0);
}

/// Every window median is the value `median_mut` picks from the pixel's replicated window, to the
/// bit: at each radius the detector uses, on frames narrower than one group of eight and wider,
/// with the values `total_cmp` orders apart from the rest — NaN of both signs, both infinities,
/// both zeros and subnormals — among random ones, so the groups of eight and the edge pixels both
/// meet them.
#[test]
fn window_medians_match_the_sorted_window_to_the_bit() {
    let mut rng = TestRng::new(31);
    let special = [
        f32::NAN,
        -f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
        0.0,
        -0.0,
        1e-40,
        -1e-40,
    ];
    for (width, height) in [(5, 4), (13, 7), (21, 13), (40, 9)] {
        let size = USize2::new(width, height);
        let data: Vec<f32> = (0..size.pixel_count())
            .map(|_| {
                let pick = rng.next_f32();
                if pick < 0.1 {
                    special[(rng.next_f32() * special.len() as f32) as usize % special.len()]
                } else if pick < 0.2 {
                    // Repeats, so equal values meet in a window.
                    0.5
                } else {
                    rng.next_f32() - 0.5
                }
            })
            .collect();
        for r in [1, 2, 3] {
            let medians = median_window(&Buffer2::new((width, height), data.clone()), r);
            for y in 0..height {
                for x in 0..width {
                    let mut window = Vec::new();
                    for dy in 0..=2 * r {
                        let yy = (y + dy).saturating_sub(r).min(height - 1);
                        for dx in 0..=2 * r {
                            let xx = (x + dx).saturating_sub(r).min(width - 1);
                            window.push(data[yy * width + xx]);
                        }
                    }
                    let expected = statistics::median_mut(&mut window);
                    assert_eq!(
                        medians[y * width + x].to_bits(),
                        expected.to_bits(),
                        "{width}x{height}, r {r}, at ({x}, {y})"
                    );
                }
            }
        }
    }
}

/// A saturated star's core, a flat top with steep edges, is L.A.Cosmic's classic false positive:
/// astroscrappy masks a saturated pixel whose 5×5 median passes a tenth of the saturation level,
/// grown 4 pixels, and so does this pass. A star of σ 1.6 and peak 40 clipped at 0.9 has a core of
/// 69 pixels to a radius of about 4.4, of which the same frame without its saturation flags marks
/// 24 cosmic rays at this seed; with them none is, while a hit that saturates a single pixel 14
/// pixels off, its 5×5 median the sky, is still found and repaired from the sky around it.
#[test]
fn a_clipped_star_core_is_kept_and_a_saturated_hit_is_not() {
    let sky = Sky {
        level: 0.05,
        noise: Some(SkyNoise {
            sigma: 0.003,
            seed: 11,
        }),
        clamp: false,
    };
    let size = USize2::new(64, 64);
    let mut data = SkyField::render(
        size,
        sky,
        GaussianStars {
            sigma: 1.6,
            peaks: &[(Vec2::new(30.0, 30.0), 40.0)],
        },
    )
    .pixels;
    let hit = USizeVec2::new(44, 30);
    data[size.idx_of(hit)] = 2.0;
    for value in data.pixels_mut() {
        *value = value.min(0.9);
    }
    let clipped: Vec<usize> = (0..size.pixel_count())
        .filter(|&idx| data.pixels()[idx] >= 0.9)
        .collect();
    let core: Vec<usize> = clipped
        .iter()
        .copied()
        .filter(|&idx| idx != size.idx_of(hit))
        .collect();
    assert_eq!(core.len(), 69);
    let rejected = |data: Buffer2<f32>, flag: bool| {
        let mut image = cfa_from_plane(data, CfaType::Mono);
        if flag {
            image.set_flags(PixelFlags::from_fn(size, |idx| {
                if clipped.contains(&idx) {
                    QualityFlags::SATURATED
                } else {
                    QualityFlags::default()
                }
            }));
        }
        reject(&mut image, &CosmicRayConfig::default()).unwrap();
        image
    };
    let cosmic = |image: &CfaImage, idx: usize| {
        image
            .flags()
            .is_some_and(|flags| flags.at(idx).intersects(QualityFlags::COSMIC_RAY))
    };

    let unflagged = rejected(data.clone(), false);
    assert_eq!(
        core.iter().filter(|&&idx| cosmic(&unflagged, idx)).count(),
        24,
        "without its flags the core is taken for a hit, or the test tells nothing"
    );

    let kept = rejected(data, true);
    for &idx in &core {
        assert!(!cosmic(&kept, idx), "core pixel {:?}", size.point_of(idx));
        assert_eq!(kept.data()[idx], 0.9);
    }
    assert!(cosmic(&kept, size.idx_of(hit)));
    assert!(
        (kept.data()[size.idx_of(hit)] - 0.05).abs() < 0.02,
        "the hit is repaired from the sky: {}",
        kept.data()[size.idx_of(hit)]
    );
}

/// An X-Trans mosaic is refused and left exactly as it was, samples and flags, before any work:
/// even a gain the frame cannot convert reports the sensor, the first thing the pass checks. Its
/// memory charge is refused the same way.
#[test]
fn an_xtrans_mosaic_is_refused_untouched() {
    let cfa = CfaType::Mosaic(Mosaic::XTrans(XTRANS_PATTERN));
    let size = USize2::new(36, 36);
    let mut data: Vec<f32> = (0..size.pixel_count())
        .map(|idx| 0.04 + 0.01 * cfa.slot_at(size.point_of(idx)) as f32)
        .collect();
    data[size.idx_of(USizeVec2::new(18, 18))] = 0.95;
    let mut image = cfa_from_plane(Buffer2::new(size, data.clone()), cfa);
    image.set_flags(PixelFlags::from_fn(size, |idx| {
        if idx == 7 {
            QualityFlags::SATURATED
        } else {
            QualityFlags::default()
        }
    }));
    let flags = image.flags().unwrap().bytes().to_vec();
    for noise in [
        NoiseEstimation::Measured,
        NoiseEstimation::Gain {
            electrons_per_adu: 1.5,
        },
    ] {
        let config = CosmicRayConfig {
            noise,
            ..Default::default()
        };
        assert_eq!(
            reject(&mut image, &config),
            Err(CosmicRayError::XTransUnsupported)
        );
        assert_eq!(bits(image.data().pixels()), bits(&data));
        assert_eq!(image.flags().unwrap().bytes(), flags);
    }
    assert_eq!(
        heap_bytes(&cfa, size),
        Err(CosmicRayError::XTransUnsupported)
    );
}

/// Recomputing only the rows a repair can move gives what recomputing every row gives, bit for bit:
/// a 96² field of noise about a sky of 0.1 with stars and hits of one to nine pixels, which the
/// detect-and-repair loop takes several passes over, cleaned both ways to the same samples and the
/// same pixels found.
#[test]
fn the_local_recompute_is_the_full_one() {
    use crate::calibration::cosmic_ray::mono::Recompute;
    use crate::calibration::cosmic_ray::mono::internals::reject_recomputing;

    let size = USize2::new(96, 96);
    let sky = Sky {
        level: 0.1,
        noise: Some(SkyNoise {
            sigma: 0.01,
            seed: 23,
        }),
        clamp: false,
    };
    let stars = [(Vec2::new(20.0, 70.0), 0.8), (Vec2::new(60.0, 30.0), 0.5)];
    let mut data = SkyField::render(
        size,
        sky,
        GaussianStars {
            sigma: 1.5,
            peaks: &stars,
        },
    )
    .pixels;
    let mut hits = Vec::new();
    for (centre, half) in [
        ((10, 10), 0),
        ((40, 50), 1),
        ((75, 75), 1),
        ((30, 88), 0),
        ((85, 12), 1),
    ] {
        for dy in 0..=2 * half {
            for dx in 0..=2 * half {
                hits.push(USizeVec2::new(centre.0 + dx - half, centre.1 + dy - half));
            }
        }
    }
    for &hit in &hits {
        data[size.idx_of(hit)] = 0.9;
    }
    let config = CosmicRayConfig::default();
    let mut local = data.clone();
    let mut full = data;
    let found_local = reject_recomputing(&mut local, &config, Recompute::ChangedRows);
    let found_full = reject_recomputing(&mut full, &config, Recompute::EveryRow);
    assert!(
        hits.iter().all(|&hit| found_full.get_at(hit)),
        "every hit is found"
    );
    assert_eq!(
        found_full.count_ones(),
        30,
        "the 29 hit pixels and one the growth takes"
    );
    assert_eq!(found_local, found_full);
    assert_eq!(bits(&local), bits(&full));
}
