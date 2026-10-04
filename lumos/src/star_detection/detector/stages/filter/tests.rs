use crate::internals::prelude::*;
use crate::star_detection::detector::stages::filter::*;
use crate::star_detection::roundness::Roundness;

#[test]
fn filter_returns_the_diagnostics_stored_by_the_detector() {
    let stars = vec![
        Star::at(DVec2::new(10.0, 10.0)).with_flux(200.0),
        Star::at(DVec2::new(11.0, 11.0)).with_flux(190.0),
        Star::at(DVec2::new(50.0, 10.0)).with_flux(180.0),
        Star::at(DVec2::new(90.0, 10.0)).with_flux(170.0),
        Star::at(DVec2::new(130.0, 10.0)).with_flux(160.0),
        Star::at(DVec2::new(170.0, 10.0)).with_flux(150.0),
        Star::at(DVec2::new(210.0, 10.0))
            .with_flux(140.0)
            .with_fwhm(20.0),
        Star::at(DVec2::new(250.0, 10.0))
            .with_flux(130.0)
            .with_saturated(true),
        Star::at(DVec2::new(290.0, 10.0))
            .with_flux(120.0)
            .with_snr(5.0),
        Star::at(DVec2::new(330.0, 10.0))
            .with_flux(110.0)
            .with_eccentricity(0.7),
        Star::at(DVec2::new(370.0, 10.0))
            .with_flux(100.0)
            .with_sharpness(0.8),
        Star::at(DVec2::new(410.0, 10.0))
            .with_flux(90.0)
            .with_roundness(Roundness {
                ground: 0.6,
                sround: 0.0,
            }),
    ];

    let outcome = FilterOutcome::from_stars(stars.clone(), &FilterConfig::default());

    assert_eq!(
        outcome
            .stars
            .iter()
            .map(|star| star.flux)
            .collect::<Vec<_>>(),
        vec![200.0, 180.0, 170.0, 160.0, 150.0]
    );
    assert_eq!(
        outcome.diagnostics,
        QualityFilterDiagnostics {
            saturated: 1,
            low_snr: 1,
            high_eccentricity: 1,
            cosmic_rays: 1,
            roundness: 1,
            fwhm_outliers: 1,
            duplicates: 1,
        }
    );

    // With no FWHM deviation bound the 20.0-px star stays, and nothing counts as an outlier.
    let unbounded = FilterOutcome::from_stars(
        stars,
        &FilterConfig {
            max_fwhm_deviation: None,
            ..FilterConfig::default()
        },
    );
    assert_eq!(
        unbounded
            .stars
            .iter()
            .map(|star| star.flux)
            .collect::<Vec<_>>(),
        vec![200.0, 180.0, 170.0, 160.0, 150.0, 140.0]
    );
    assert_eq!(unbounded.diagnostics.fwhm_outliers, 0);
}

/// FWHM outlier rejection over every case that mattered, as one table.
///
/// Each row pins the *surviving fluxes in order*. Fluxes are distinct and exactly representable,
/// so the sequence identifies precisely which stars survived, and in what order.
///
/// The reference set is the first `max(len / 2, 5)` stars *in the order given*, which is why every
/// fixture is built brightest-first: production sorts by flux before calling this. `max_fwhm` is
/// `median + deviation · max(mad, median · 0.1)`, and the floor is what stops an all-identical
/// reference from rejecting everything.
#[test]
fn filter_fwhm_outliers_over_every_case() {
    /// One `filter_fwhm_outliers` call and the stars it must leave behind.
    struct Case {
        name: &'static str,
        /// `(fwhm, flux)` pairs, brightest first.
        stars: Vec<(f32, f32)>,
        deviation: f32,
        /// Surviving fluxes, in order.
        survivors: Vec<f32>,
    }

    /// `(fwhm, flux)` pairs, brightest first.
    fn stars(pairs: &[(f32, f32)]) -> Vec<Star> {
        pairs
            .iter()
            .map(|&(fwhm, flux)| Star::at(DVec2::ZERO).with_fwhm(fwhm).with_flux(flux))
            .collect()
    }
    /// `count` stars starting at `fwhm`, stepping by `step`, fluxes descending from 100.
    fn ramp(count: usize, fwhm: f32, step: f32) -> Vec<(f32, f32)> {
        (0..count)
            .map(|i| (fwhm + i as f32 * step, 100.0 - i as f32))
            .collect()
    }
    fn fluxes(from: f32, count: usize) -> Vec<f32> {
        (0..count).map(|i| from - i as f32).collect()
    }

    let mixed_flux_order = [
        (3.0, 100.0),
        (3.1, 95.0),
        (2.9, 90.0),
        (3.2, 85.0),
        (3.0, 80.0),
        (3.5, 50.0),
        (4.0, 40.0),
        (8.0, 30.0),
        (3.1, 20.0),
        (15.0, 10.0),
    ];
    let mut with_two_outliers = ramp(8, 3.0, 0.2);
    with_two_outliers.extend([(6.0, 10.0), (7.0, 5.0)]);

    let cases = vec![
        Case {
            name: "four stars",
            stars: ramp(4, 3.0, 10.0),
            deviation: 3.0,
            survivors: fluxes(100.0, 4),
        },
        // Exactly five is the smallest set that filters. Reference is all five: median 3.1,
        // mad 0.1, floor 0.31, σ = 1.4826·0.31 = 0.4596, so max_fwhm = 3.1 + 3·0.4596 = 4.48 and
        // only the 20.0 goes.
        Case {
            name: "exactly five stars",
            stars: vec![
                (3.0, 100.0),
                (3.1, 90.0),
                (3.0, 80.0),
                (3.2, 70.0),
                (20.0, 60.0),
            ],
            deviation: 3.0,
            survivors: vec![100.0, 90.0, 80.0, 70.0],
        },
        // Reference 3.0..3.4: median 3.2, mad 0.1, floor 0.32, σ 0.4744 → max_fwhm 4.62.
        Case {
            name: "one gross outlier",
            stars: {
                let mut s = ramp(9, 3.0, 0.1);
                s.push((20.0, 10.0));
                s
            },
            deviation: 3.0,
            survivors: fluxes(100.0, 9),
        },
        Case {
            name: "three gross outliers",
            stars: {
                let mut s = ramp(7, 3.0, 0.1);
                s.extend([(15.0, 5.0), (18.0, 4.0), (25.0, 3.0)]);
                s
            },
            deviation: 3.0,
            survivors: fluxes(100.0, 7),
        },
        // Nothing stands out: reference median 3.1, floor 0.31, σ 0.4596 → max_fwhm 4.48, and the
        // widest star is 3.45.
        Case {
            name: "uniform",
            stars: ramp(10, 3.0, 0.05),
            deviation: 3.0,
            survivors: fluxes(100.0, 10),
        },
        // An identical reference gives mad = 0, so only the floor keeps the threshold finite:
        // median 3.0, floor 0.3, σ 0.4448 → max_fwhm 4.33, which the 5.0 exceeds.
        Case {
            name: "mad floor carries a zero-spread reference",
            stars: {
                let mut s: Vec<(f32, f32)> = (0..9).map(|i| (3.0, 100.0 - i as f32)).collect();
                s.push((5.0, 10.0));
                s
            },
            deviation: 3.0,
            survivors: fluxes(100.0, 9),
        },
        // The reference is the bright half only, so the faint half is judged against it: median
        // 3.0, mad 0.1, floor 0.3, σ 0.4448 → max_fwhm 4.33. The 4.0 stays, the 8.0 and the 15.0
        // go.
        Case {
            name: "reference is the bright half",
            stars: mixed_flux_order.to_vec(),
            deviation: 3.0,
            survivors: vec![100.0, 95.0, 90.0, 85.0, 80.0, 50.0, 40.0, 20.0],
        },
        // One fixture, two deviations. Reference 3.0..3.8: median 3.4, mad 0.2, floor 0.34,
        // σ 0.5041. Strict 1.5 → max_fwhm 4.16, keeping six of the ramp to 4.4; loose 5.0 → 5.92,
        // keeping all but 6.0 and 7.0.
        Case {
            name: "strict deviation",
            stars: with_two_outliers.clone(),
            deviation: 1.5,
            survivors: fluxes(100.0, 6),
        },
        Case {
            name: "loose deviation",
            stars: with_two_outliers,
            deviation: 5.0,
            survivors: fluxes(100.0, 8),
        },
    ];

    for case in cases {
        let Case {
            name,
            stars: pairs,
            deviation,
            survivors: expected,
        } = case;
        let mut subjects = stars(&pairs);
        let before = subjects.len();
        let removed = filter_fwhm_outliers(&mut subjects, deviation);

        let survivors: Vec<f32> = subjects.iter().map(|s| s.flux).collect();
        assert_eq!(survivors, expected, "{name}: surviving fluxes");
        assert_eq!(
            removed,
            before - expected.len(),
            "{name}: reported removal count must match what is gone"
        );
    }
}

/// Deduplication over every geometry that mattered, as one table, through both the brute-force
/// and the spatial-hash path — the dispatcher picks by star count, so neither path alone would
/// see every row.
///
/// Each case pins the *surviving fluxes in order*. Fluxes are distinct within every case, so the
/// expected sequence identifies exactly which stars survived and in what order.
#[test]
fn remove_duplicate_stars_over_every_geometry() {
    type Path = fn(&mut Vec<Star>, f32) -> usize;

    struct Case {
        /// `(x, y, flux)` in input order — the order the function actually honours.
        stars: Vec<(f64, f64, f32)>,
        separation: f32,
        /// Fluxes of the survivors, in order.
        survivors: Vec<f32>,
        why: &'static str,
    }

    let cases = [
        Case {
            stars: vec![],
            separation: 8.0,
            survivors: vec![],
            why: "nothing to dedupe",
        },
        Case {
            stars: vec![(10.0, 10.0, 100.0)],
            separation: 8.0,
            survivors: vec![100.0],
            why: "one star is never its own duplicate",
        },
        Case {
            stars: vec![
                (10.0, 10.0, 100.0),
                (50.0, 50.0, 90.0),
                (100.0, 100.0, 80.0),
            ],
            separation: 8.0,
            survivors: vec![100.0, 90.0, 80.0],
            why: "all far apart",
        },
        Case {
            stars: vec![(10.0, 10.0, 100.0), (12.0, 12.0, 90.0), (50.0, 50.0, 80.0)],
            separation: 8.0,
            survivors: vec![100.0, 80.0],
            why: "one pair at 2.83, one star far off",
        },
        Case {
            stars: vec![(10.0, 10.0, 100.0), (11.0, 11.0, 50.0)],
            separation: 8.0,
            survivors: vec![100.0],
            why: "brightest-first input keeps the bright one",
        },
        Case {
            // The documented precondition: it keeps the FIRST of a cluster and never reads
            // `.flux`. Callers sort by flux beforehand; this is what skipping that gets you.
            stars: vec![(11.0, 11.0, 50.0), (10.0, 10.0, 100.0)],
            separation: 8.0,
            survivors: vec![50.0],
            why: "unsorted input keeps first, not brightest",
        },
        Case {
            stars: vec![(10.0, 10.0, 100.0), (16.0, 16.0, 90.0)],
            separation: 8.0,
            survivors: vec![100.0, 90.0],
            why: "sqrt(6^2+6^2) = 8.485, outside 8.0",
        },
        Case {
            // The boundary itself. The comparison is strictly less than, so a pair exactly
            // `separation` apart survives; a hair inside it does not.
            stars: vec![(10.0, 10.0, 100.0), (18.0, 10.0, 90.0)],
            separation: 8.0,
            survivors: vec![100.0, 90.0],
            why: "distance == separation is kept",
        },
        Case {
            stars: vec![(10.0, 10.0, 100.0), (17.999, 10.0, 90.0)],
            separation: 8.0,
            survivors: vec![100.0],
            why: "a hair inside the boundary is removed",
        },
        Case {
            stars: vec![(10.0, 10.0, 100.0), (15.0, 15.0, 90.0)],
            separation: 8.0,
            survivors: vec![100.0],
            why: "sqrt(5^2+5^2) = 7.07, inside 8.0",
        },
        Case {
            stars: vec![(10.0, 10.0, 100.0), (12.0, 10.0, 90.0), (14.0, 10.0, 80.0)],
            separation: 8.0,
            survivors: vec![100.0],
            why: "cluster of three collapses to the first",
        },
        Case {
            stars: vec![
                (10.0, 10.0, 100.0),
                (12.0, 10.0, 90.0),
                (100.0, 100.0, 80.0),
                (102.0, 100.0, 70.0),
            ],
            separation: 8.0,
            survivors: vec![100.0, 80.0],
            why: "two pairs, far apart from each other",
        },
        Case {
            // Chained: 5 is inside 8 of the first, 10 is not, and 20 is clear of 10.
            stars: vec![
                (0.0, 0.0, 100.0),
                (5.0, 0.0, 90.0),
                (10.0, 0.0, 80.0),
                (20.0, 0.0, 70.0),
            ],
            separation: 8.0,
            survivors: vec![100.0, 80.0, 70.0],
            why: "a removed star cannot shadow the next",
        },
        Case {
            // Twenty stars 0.5 px apart from x = 10: those up to 17.5 are inside 8 of the first,
            // the one at 18.0 is exactly 8 away and stays, and 18.5..19.5 fall inside 8 of it.
            stars: (0..20)
                .map(|i| (10.0 + f64::from(i) * 0.5, 10.0, 100.0 - i as f32))
                .collect(),
            separation: 8.0,
            survivors: vec![100.0, 84.0],
            why: "a dense run keeps its first and the first star 8 px on",
        },
        Case {
            stars: vec![(10.0, 10.0, 100.0), (10.0, 15.0, 90.0), (10.0, 25.0, 80.0)],
            separation: 8.0,
            survivors: vec![100.0, 80.0],
            why: "separation is euclidean, not per-axis",
        },
        Case {
            stars: vec![(10.0, 10.0, 100.0), (10.0, 10.0, 90.0), (10.0, 10.0, 80.0)],
            separation: 8.0,
            survivors: vec![100.0],
            why: "coincident stars collapse to one",
        },
        Case {
            stars: vec![(10.0, 10.0, 100.0), (30.0, 10.0, 90.0), (50.0, 10.0, 80.0)],
            separation: 25.0,
            survivors: vec![100.0, 80.0],
            why: "20 < 25 removed, 40 >= 25 kept",
        },
        Case {
            // Cells are `separation` wide: x = 49 and 51 sit in cells 9 and 10 at separation 5.
            stars: vec![(49.0, 50.0, 100.0), (51.0, 50.0, 90.0)],
            separation: 5.0,
            survivors: vec![100.0],
            why: "a duplicate across a cell boundary",
        },
        Case {
            stars: vec![(49.0, 49.0, 100.0), (51.0, 51.0, 90.0), (56.0, 49.0, 80.0)],
            separation: 5.0,
            survivors: vec![100.0, 80.0],
            why: "a duplicate in the diagonal cell; 7 px on, two cells over, is kept",
        },
        Case {
            stars: vec![
                (10.0, 10.0, 100.0),
                (12.0, 10.0, 95.0),
                (50.0, 50.0, 90.0),
                (100.0, 100.0, 85.0),
            ],
            separation: 8.0,
            survivors: vec![100.0, 90.0, 85.0],
            why: "survivors keep their input order",
        },
    ];

    let paths: [(&str, Path); 2] = [
        ("brute force", brute_force_dedup),
        ("cells", remove_duplicate_stars),
    ];
    for case in &cases {
        for (path, dedupe) in paths {
            let mut stars: Vec<Star> = case
                .stars
                .iter()
                .map(|&(x, y, flux)| Star::at(DVec2::new(x, y)).with_flux(flux))
                .collect();
            let removed = dedupe(&mut stars, case.separation);

            let survivors: Vec<f32> = stars.iter().map(|s| s.flux).collect();
            assert_eq!(survivors, case.survivors, "{path}: {}", case.why);
            assert_eq!(
                removed,
                case.stars.len() - case.survivors.len(),
                "{path}: {}: removed count disagrees with the survivors",
                case.why
            );
        }
    }
}

/// `duplicate_min_separation = 0` passes validation and means "no deduplication", at any count —
/// the cells must not divide every coordinate by a zero size.
#[test]
fn zero_separation_removes_nothing() {
    for count in [10, 150] {
        // Every star twice, at exactly the same spot.
        let mut stars: Vec<Star> = (0..count)
            .map(|i| Star::at(DVec2::new((i / 2) as f64 * 3.0, 7.0)).with_flux(1.0))
            .collect();
        assert_eq!(remove_duplicate_stars(&mut stars, 0.0), 0, "{count} stars");
        assert_eq!(stars.len(), count);
    }
}

/// The cell pass agrees star for star with the brute force on 500 random positions.
#[test]
fn remove_duplicate_stars_matches_the_brute_force() {
    let mut rng = TestRng::new(12345);
    let base_stars: Vec<Star> = (0..500)
        .map(|i| {
            let x = f64::from(rng.next_f32() * 1000.0);
            let y = f64::from(rng.next_f32() * 1000.0);
            Star::at(DVec2::new(x, y)).with_flux(1000.0 - i as f32)
        })
        .collect();

    let mut stars_hash = base_stars.clone();
    let removed_hash = remove_duplicate_stars(&mut stars_hash, 10.0);
    let mut stars_simple = base_stars;
    let removed_simple = brute_force_dedup(&mut stars_simple, 10.0);

    assert!(
        removed_hash > 0,
        "the fixture must have duplicates to agree on"
    );
    assert_eq!(removed_hash, removed_simple);
    let positions = |stars: &[Star]| stars.iter().map(|s| s.pos).collect::<Vec<_>>();
    assert_eq!(positions(&stars_hash), positions(&stars_simple));
}

/// Each quality test on its bound passes — every criterion rejects strictly beyond it — and a star
/// failing two criteria is counted under the first one checked: saturation, SNR, eccentricity,
/// sharpness, roundness.
#[test]
fn rejection_bounds_and_precedence() {
    let config = FilterConfig::default();
    let base = Star::at(DVec2::ZERO);
    let round = |ground: f32| Roundness {
        ground,
        sround: 0.0,
    };
    let cases = [
        (base.with_snr(config.min_snr), None),
        (base.with_eccentricity(config.max_eccentricity), None),
        (base.with_sharpness(config.max_sharpness), None),
        (base.with_roundness(round(config.max_roundness)), None),
        (base.with_roundness(round(-config.max_roundness)), None),
        (base.with_snr(9.99), Some(Rejection::LowSnr)),
        (base.with_eccentricity(0.61), Some(Rejection::Eccentric)),
        (base.with_sharpness(0.71), Some(Rejection::CosmicRay)),
        (base.with_roundness(round(-0.51)), Some(Rejection::NotRound)),
        (
            base.with_saturated(true).with_snr(1.0),
            Some(Rejection::Saturated),
        ),
        (
            base.with_snr(1.0).with_eccentricity(0.9),
            Some(Rejection::LowSnr),
        ),
        (
            base.with_eccentricity(0.9).with_sharpness(0.9),
            Some(Rejection::Eccentric),
        ),
        (
            base.with_sharpness(0.9).with_roundness(round(0.9)),
            Some(Rejection::CosmicRay),
        ),
    ];
    for (star, expected) in cases {
        assert_eq!(Rejection::of(&star, &config), expected, "{star:?}");
    }
}

/// A NaN flux sorts last, and the finite ones go brightest first. The old comparator called NaN
/// equal to everything, which is not an order.
#[test]
fn sort_by_flux_puts_nan_last() {
    let mut stars: Vec<Star> = [3.0, f32::NAN, 7.0, -1.0, f32::NAN, 5.0]
        .into_iter()
        .map(|flux| Star::at(DVec2::ZERO).with_flux(flux))
        .collect();
    sort_by_flux(&mut stars);
    let fluxes: Vec<f32> = stars.iter().map(|star| star.flux).collect();
    assert_eq!(&fluxes[..4], &[7.0, 5.0, 3.0, -1.0]);
    assert!(fluxes[4].is_nan() && fluxes[5].is_nan());
}

/// The reference the cell pass is held to: every pair, O(n²), each kept star dropping the later
/// stars strictly closer than `min_separation`.
fn brute_force_dedup(stars: &mut Vec<Star>, min_separation: f32) -> usize {
    let min_sep_sq = f64::from(min_separation * min_separation);
    let mut kept = vec![true; stars.len()];

    for i in 0..stars.len() {
        if !kept[i] {
            continue;
        }
        for j in (i + 1)..stars.len() {
            if !kept[j] {
                continue;
            }
            let dx = stars[i].pos.x - stars[j].pos.x;
            let dy = stars[i].pos.y - stars[j].pos.y;
            if dx * dx + dy * dy < min_sep_sq {
                kept[j] = false;
            }
        }
    }

    let removed = kept.iter().filter(|&&keep| !keep).count();
    let mut index = 0;
    stars.retain(|_| {
        index += 1;
        kept[index - 1]
    });
    removed
}
