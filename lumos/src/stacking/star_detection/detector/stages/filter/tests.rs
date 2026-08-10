use crate::stacking::star_detection::detector::stages::filter::*;
use crate::stacking::star_detection::roundness::Roundness;
use crate::testing::prelude::*;

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
            .with_peak(0.96),
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

    let outcome = FilterOutcome::from_stars(stars, &FilterConfig::default());

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
}

/// FWHM outlier rejection over every case that mattered, as one table.
///
/// Each row pins the *surviving fluxes in order*, which is stronger than the removed-count the
/// eleven tests this replaces asserted. Fluxes are distinct and exactly representable, so the
/// sequence identifies precisely which stars survived — and because it is a sequence, it also
/// subsumes the separate order-preservation test.
///
/// The reference set is the first `max(len / 2, 5)` stars *in the order given*, which is why every
/// fixture is built brightest-first: production sorts by flux before calling this. `max_fwhm` is
/// `median + deviation · max(mad, median · 0.1)`, and the floor is what stops an all-identical
/// reference from rejecting everything.
#[test]
fn filter_fwhm_outliers_over_every_case() {
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

    /// One `filter_fwhm_outliers` call and the stars it must leave behind.
    struct Case {
        name: &'static str,
        /// `(fwhm, flux)` pairs, brightest first.
        stars: Vec<(f32, f32)>,
        deviation: f32,
        /// Surviving fluxes, in order.
        survivors: Vec<f32>,
    }

    let cases = vec![
        // Both disabling conditions: a non-positive deviation short-circuits before any statistic
        // is computed, so even a wildly spread set survives intact.
        Case {
            name: "zero deviation",
            stars: ramp(10, 3.0, 1.0),
            deviation: 0.0,
            survivors: fluxes(100.0, 10),
        },
        Case {
            name: "negative deviation",
            stars: ramp(10, 3.0, 5.0),
            deviation: -1.0,
            survivors: fluxes(100.0, 10),
        },
        // Under five stars there is no reference to speak of, so nothing is filtered.
        Case {
            name: "four stars",
            stars: ramp(4, 3.0, 10.0),
            deviation: 3.0,
            survivors: fluxes(100.0, 4),
        },
        // Exactly five is the smallest set that filters. Reference is all five: median 3.1,
        // mad 0.1, floor 0.31, so max_fwhm = 3.1 + 3·0.31 = 4.03 and only the 20.0 goes.
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
        // Reference 3.0..3.4: median 3.2, mad 0.1, floor 0.32 → max_fwhm 4.16.
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
        // Nothing stands out: reference median 3.1, floor 0.31 → max_fwhm 4.03, and the widest
        // star is 3.45.
        Case {
            name: "uniform",
            stars: ramp(10, 3.0, 0.05),
            deviation: 3.0,
            survivors: fluxes(100.0, 10),
        },
        // An identical reference gives mad = 0, so only the floor keeps the threshold finite:
        // median 3.0, floor 0.3 → max_fwhm 3.9, which the 5.0 exceeds.
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
        // 3.0, mad 0.1, floor 0.3 → max_fwhm 3.9. The 4.0 goes with the 8.0 and the 15.0.
        Case {
            name: "reference is the bright half",
            stars: mixed_flux_order.to_vec(),
            deviation: 3.0,
            survivors: vec![100.0, 95.0, 90.0, 85.0, 80.0, 50.0, 20.0],
        },
        // One fixture, two deviations. Reference 3.0..3.8: median 3.4, mad 0.2, floor 0.34.
        // Strict 1.5 → max_fwhm 3.91, keeping five; loose 5.0 → 5.10, keeping all but 6.0 and 7.0.
        Case {
            name: "strict deviation",
            stars: with_two_outliers.clone(),
            deviation: 1.5,
            survivors: fluxes(100.0, 5),
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

/// Deduplication over every geometry that mattered, as one table.
///
/// Each case pins the *surviving fluxes in order*, which is stronger than what most of the
/// fifteen tests this replaces asserted — they checked a count, and sometimes one coordinate.
/// Fluxes are distinct within every case, so the expected sequence identifies exactly which
/// stars survived and in what order.
#[test]
fn remove_duplicate_stars_over_every_geometry() {
    struct Case {
        /// `(x, y, flux)` in input order — the order the function actually honours.
        stars: &'static [(f64, f64, f32)],
        separation: f32,
        /// Fluxes of the survivors, in order.
        survivors: &'static [f32],
        why: &'static str,
    }

    let cases = [
        Case {
            stars: &[],
            separation: 8.0,
            survivors: &[],
            why: "nothing to dedupe",
        },
        Case {
            stars: &[(10.0, 10.0, 100.0)],
            separation: 8.0,
            survivors: &[100.0],
            why: "one star is never its own duplicate",
        },
        Case {
            stars: &[
                (10.0, 10.0, 100.0),
                (50.0, 50.0, 90.0),
                (100.0, 100.0, 80.0),
            ],
            separation: 8.0,
            survivors: &[100.0, 90.0, 80.0],
            why: "all far apart",
        },
        Case {
            stars: &[(10.0, 10.0, 100.0), (12.0, 12.0, 90.0), (50.0, 50.0, 80.0)],
            separation: 8.0,
            survivors: &[100.0, 80.0],
            why: "one pair at 2.83, one star far off",
        },
        Case {
            stars: &[(10.0, 10.0, 100.0), (11.0, 11.0, 50.0)],
            separation: 8.0,
            survivors: &[100.0],
            why: "brightest-first input keeps the bright one",
        },
        Case {
            // The documented precondition: it keeps the FIRST of a cluster and never reads
            // `.flux`. Callers sort by flux beforehand; this is what skipping that gets you.
            stars: &[(11.0, 11.0, 50.0), (10.0, 10.0, 100.0)],
            separation: 8.0,
            survivors: &[50.0],
            why: "unsorted input keeps first, not brightest",
        },
        Case {
            stars: &[(10.0, 10.0, 100.0), (16.0, 16.0, 90.0)],
            separation: 8.0,
            survivors: &[100.0, 90.0],
            why: "sqrt(6^2+6^2) = 8.485, outside 8.0",
        },
        Case {
            // The boundary itself. The comparison is strictly less than, so a pair exactly
            // `separation` apart survives; a hair inside it does not.
            stars: &[(10.0, 10.0, 100.0), (18.0, 10.0, 90.0)],
            separation: 8.0,
            survivors: &[100.0, 90.0],
            why: "distance == separation is kept",
        },
        Case {
            stars: &[(10.0, 10.0, 100.0), (17.999, 10.0, 90.0)],
            separation: 8.0,
            survivors: &[100.0],
            why: "a hair inside the boundary is removed",
        },
        Case {
            stars: &[(10.0, 10.0, 100.0), (15.0, 15.0, 90.0)],
            separation: 8.0,
            survivors: &[100.0],
            why: "sqrt(5^2+5^2) = 7.07, inside 8.0",
        },
        Case {
            stars: &[(10.0, 10.0, 100.0), (12.0, 10.0, 90.0), (14.0, 10.0, 80.0)],
            separation: 8.0,
            survivors: &[100.0],
            why: "cluster of three collapses to the first",
        },
        Case {
            stars: &[
                (10.0, 10.0, 100.0),
                (12.0, 10.0, 90.0),
                (100.0, 100.0, 80.0),
                (102.0, 100.0, 70.0),
            ],
            separation: 8.0,
            survivors: &[100.0, 80.0],
            why: "two pairs, far apart from each other",
        },
        Case {
            // Chained: 5 is inside 8 of the first, 10 is not, and 20 is clear of 10.
            stars: &[
                (0.0, 0.0, 100.0),
                (5.0, 0.0, 90.0),
                (10.0, 0.0, 80.0),
                (20.0, 0.0, 70.0),
            ],
            separation: 8.0,
            survivors: &[100.0, 80.0, 70.0],
            why: "a removed star cannot shadow the next",
        },
        Case {
            stars: &[(10.0, 10.0, 100.0), (10.0, 15.0, 90.0), (10.0, 25.0, 80.0)],
            separation: 8.0,
            survivors: &[100.0, 80.0],
            why: "separation is euclidean, not per-axis",
        },
        Case {
            stars: &[(10.0, 10.0, 100.0), (10.0, 10.0, 90.0), (10.0, 10.0, 80.0)],
            separation: 8.0,
            survivors: &[100.0],
            why: "coincident stars collapse to one",
        },
        Case {
            stars: &[(10.0, 10.0, 100.0), (30.0, 10.0, 90.0), (50.0, 10.0, 80.0)],
            separation: 25.0,
            survivors: &[100.0, 80.0],
            why: "20 < 25 removed, 40 >= 25 kept",
        },
        Case {
            stars: &[
                (10.0, 10.0, 100.0),
                (12.0, 10.0, 95.0),
                (50.0, 50.0, 90.0),
                (100.0, 100.0, 85.0),
            ],
            separation: 8.0,
            survivors: &[100.0, 90.0, 85.0],
            why: "survivors keep their input order",
        },
    ];

    for case in &cases {
        let mut stars: Vec<Star> = case
            .stars
            .iter()
            .map(|&(x, y, flux)| Star::at(DVec2::new(x, y)).with_flux(flux))
            .collect();
        let removed = remove_duplicate_stars(&mut stars, case.separation);

        let survivors: Vec<f32> = stars.iter().map(|s| s.flux).collect();
        assert_eq!(survivors, case.survivors, "{}", case.why);
        assert_eq!(
            removed,
            case.stars.len() - case.survivors.len(),
            "{}: removed count disagrees with the survivors",
            case.why
        );
    }
}

#[test]
fn remove_duplicate_stars_many_duplicates() {
    // 20 stars along x=10..19.5, y=10, spacing=0.5px, all within 8px of star[0]
    // Star[0] at x=10 has highest flux (100), so it survives.
    // Stars at x=10.5..19.5 are within 9.5px of star[0].
    // All stars within 8px of any brighter star get removed.
    // Star at x=18.0 is 8.0px from star[0] — at boundary (not removed since
    // distance must be strictly less). But star at x=17.5 is 7.5 < 8.0 → removed.
    let mut stars: Vec<Star> = (0..20)
        .map(|i| Star::at(DVec2::new(10.0 + (i as f64 * 0.5), 10.0)).with_flux(100.0 - i as f32))
        .collect();

    let removed = remove_duplicate_stars(&mut stars, 8.0);

    // Star[0] (x=10.0, flux=100): kept.
    // Stars[1..16] (x=10.5..17.5): dist < 8.0 from star[0] → removed (15 stars).
    // Star[16] (x=18.0, flux=84): dist = 8.0 from star[0]. 8^2 = 64 is NOT < 64 → kept.
    // Stars[17..20] (x=18.5..19.5): dist < 8.0 from star[16] → removed (3 stars).
    // Total: 15 + 3 = 18 removed, 2 survivors.
    assert_eq!(
        removed, 18,
        "Should remove 18 of 20 clustered stars, removed {}",
        removed
    );
    assert_eq!(stars.len(), 2, "Star[0] and star[16] should survive");
}

#[test]
fn remove_duplicate_stars_spatial_hash_path() {
    // Test with >100 stars to exercise spatial hashing code path
    // Create a grid of stars with some duplicates
    let mut stars: Vec<Star> = Vec::new();

    // Create 150 stars in a grid pattern (15x10)
    for y in 0..10 {
        for x in 0..15 {
            let px = x as f64 * 20.0 + 10.0; // 20 pixel spacing
            let py = y as f64 * 20.0 + 10.0;
            let flux = 1000.0 - (y * 15 + x) as f32; // Decreasing flux
            stars.push(Star::at(DVec2::new(px, py)).with_flux(flux));
        }
    }

    // Add some duplicates close to existing stars
    stars.push(Star::at(DVec2::new(12.0, 12.0)).with_flux(50.0)); // Close to (10, 10)
    stars.push(Star::at(DVec2::new(32.0, 12.0)).with_flux(45.0)); // Close to (30, 10)
    stars.push(Star::at(DVec2::new(52.0, 32.0)).with_flux(40.0)); // Close to (50, 30)

    // Sort by flux (required)
    stars.sort_by(|a, b| b.flux.partial_cmp(&a.flux).unwrap());

    let initial_count = stars.len();
    let removed = remove_duplicate_stars(&mut stars, 8.0);

    // Should remove the 3 duplicates
    assert_eq!(removed, 3);
    assert_eq!(stars.len(), initial_count - 3);

    // Verify no remaining stars are too close
    for i in 0..stars.len() {
        for j in (i + 1)..stars.len() {
            let dx = stars[i].pos.x - stars[j].pos.x;
            let dy = stars[i].pos.y - stars[j].pos.y;
            let dist_sq = dx * dx + dy * dy;
            assert!(
                dist_sq >= 64.0, // 8.0^2
                "Stars at ({}, {}) and ({}, {}) are too close: dist={}",
                stars[i].pos.x,
                stars[i].pos.y,
                stars[j].pos.x,
                stars[j].pos.y,
                dist_sq.sqrt()
            );
        }
    }
}

#[test]
fn remove_duplicate_stars_spatial_hash_edge_cases() {
    // Test edge cases for spatial hashing: stars at grid cell boundaries
    let mut stars: Vec<Star> = Vec::new();

    // Create 200 stars spread across a large area
    for i in 0..200 {
        let x = (i % 20) as f64 * 100.0 + 50.0;
        let y = (i / 20) as f64 * 100.0 + 50.0;
        stars.push(Star::at(DVec2::new(x, y)).with_flux(1000.0 - i as f32));
    }

    // Add duplicates at cell boundaries (separation = 5.0, so cell size = 5.0)
    // Star at boundary between cells
    stars.push(Star::at(DVec2::new(52.0, 50.0)).with_flux(10.0)); // Close to (50, 50)

    stars.sort_by(|a, b| b.flux.partial_cmp(&a.flux).unwrap());

    let removed = remove_duplicate_stars(&mut stars, 5.0);

    assert_eq!(removed, 1);
    assert_eq!(stars.len(), 200);
}

#[test]
fn remove_duplicate_stars_spatial_hash_consistency() {
    // Verify spatial hash gives same results as simple algorithm
    let mut rng = TestRng::new(12345);

    // Generate 500 random stars. The positions stay f32-derived so the RNG stream, and with it
    // the fixture, is unchanged by the widening to DVec2.
    let base_stars: Vec<Star> = (0..500)
        .map(|i| {
            let x = (rng.next_f32() * 1000.0) as f64;
            let y = (rng.next_f32() * 1000.0) as f64;
            Star::at(DVec2::new(x, y)).with_flux(1000.0 - i as f32)
        })
        .collect();

    // Run with spatial hash (>100 stars)
    let mut stars_hash = base_stars.clone();
    stars_hash.sort_by(|a, b| b.flux.partial_cmp(&a.flux).unwrap());
    let removed_hash = remove_duplicate_stars(&mut stars_hash, 10.0);

    // Run with simple algorithm (force by using small chunks)
    let mut stars_simple = base_stars;
    stars_simple.sort_by(|a, b| b.flux.partial_cmp(&a.flux).unwrap());
    let removed_simple = remove_duplicate_stars_simple(&mut stars_simple, 10.0);

    // Results should match
    assert_eq!(
        removed_hash, removed_simple,
        "Spatial hash removed {} but simple removed {}",
        removed_hash, removed_simple
    );
    assert_eq!(stars_hash.len(), stars_simple.len());

    // Verify same stars kept (by position)
    for (h, s) in stars_hash.iter().zip(stars_simple.iter()) {
        assert!(
            (h.pos.x - s.pos.x).abs() < 0.001 && (h.pos.y - s.pos.y).abs() < 0.001,
            "Mismatch: hash({}, {}) vs simple({}, {})",
            h.pos.x,
            h.pos.y,
            s.pos.x,
            s.pos.y
        );
    }
}
