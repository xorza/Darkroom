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

#[test]
fn filter_fwhm_outliers_disabled_when_zero_deviation() {
    // `filter_fwhm_outliers` never reads position, so every fixture below sits at the origin.
    let mut stars: Vec<Star> = (0..10)
        .map(|i| {
            Star::at(DVec2::ZERO)
                .with_fwhm(3.0 + i as f32)
                .with_flux(100.0 - i as f32)
        })
        .collect();

    let removed = filter_fwhm_outliers(&mut stars, 0.0);

    assert_eq!(removed, 0);
    assert_eq!(stars.len(), 10);
}

#[test]
fn filter_fwhm_outliers_disabled_when_too_few_stars() {
    let mut stars: Vec<Star> = (0..4)
        .map(|i| {
            Star::at(DVec2::ZERO)
                .with_fwhm(3.0 + i as f32 * 10.0)
                .with_flux(100.0 - i as f32)
        })
        .collect();

    let removed = filter_fwhm_outliers(&mut stars, 3.0);

    assert_eq!(removed, 0);
    assert_eq!(stars.len(), 4);
}

#[test]
fn filter_fwhm_outliers_removes_single_outlier() {
    // 9 stars with FWHM ~3.0, 1 star with FWHM 20.0
    let mut stars: Vec<Star> = (0..9)
        .map(|i| {
            Star::at(DVec2::ZERO)
                .with_fwhm(3.0 + (i as f32 * 0.1))
                .with_flux(100.0 - i as f32)
        })
        .collect();
    // Outlier with low flux
    stars.push(Star::at(DVec2::ZERO).with_fwhm(20.0).with_flux(10.0));

    let removed = filter_fwhm_outliers(&mut stars, 3.0);

    assert_eq!(removed, 1);
    assert_eq!(stars.len(), 9);
    assert!(stars.iter().all(|s| s.fwhm < 10.0));
}

#[test]
fn filter_fwhm_outliers_removes_multiple_outliers() {
    // 7 stars with FWHM ~3.0, 3 stars with FWHM > 15.0
    let mut stars: Vec<Star> = (0..7)
        .map(|i| {
            Star::at(DVec2::ZERO)
                .with_fwhm(3.0 + (i as f32 * 0.1))
                .with_flux(100.0 - i as f32)
        })
        .collect();
    stars.push(Star::at(DVec2::ZERO).with_fwhm(15.0).with_flux(5.0));
    stars.push(Star::at(DVec2::ZERO).with_fwhm(18.0).with_flux(4.0));
    stars.push(Star::at(DVec2::ZERO).with_fwhm(25.0).with_flux(3.0));

    let removed = filter_fwhm_outliers(&mut stars, 3.0);

    assert_eq!(removed, 3);
    assert_eq!(stars.len(), 7);
}

#[test]
fn filter_fwhm_outliers_keeps_all_when_uniform() {
    // All stars have similar FWHM
    let mut stars: Vec<Star> = (0..10)
        .map(|i| {
            Star::at(DVec2::ZERO)
                .with_fwhm(3.0 + (i as f32 * 0.05))
                .with_flux(100.0 - i as f32)
        })
        .collect();

    let removed = filter_fwhm_outliers(&mut stars, 3.0);

    assert_eq!(removed, 0);
    assert_eq!(stars.len(), 10);
}

#[test]
fn filter_fwhm_outliers_uses_effective_mad_floor() {
    // All identical FWHM values -> MAD = 0, but effective_mad = median * 0.1
    // With median = 3.0, effective_mad = 0.3
    // max_fwhm = 3.0 + 3.0 * 0.3 = 3.9
    let mut stars: Vec<Star> = (0..9)
        .map(|i| {
            Star::at(DVec2::ZERO)
                .with_fwhm(3.0)
                .with_flux(100.0 - i as f32)
        })
        .collect();
    // Should be removed (5.0 > 3.9)
    stars.push(Star::at(DVec2::ZERO).with_fwhm(5.0).with_flux(10.0));

    let removed = filter_fwhm_outliers(&mut stars, 3.0);

    assert_eq!(removed, 1);
    assert_eq!(stars.len(), 9);
}

#[test]
fn filter_fwhm_outliers_uses_top_half_for_reference() {
    // First 5 stars (top half by flux) have FWHM ~3.0
    // Last 5 stars have varying FWHM including outliers
    let mut stars: Vec<Star> = vec![
        Star::at(DVec2::ZERO).with_fwhm(3.0).with_flux(100.0),
        Star::at(DVec2::ZERO).with_fwhm(3.1).with_flux(95.0),
        Star::at(DVec2::ZERO).with_fwhm(2.9).with_flux(90.0),
        Star::at(DVec2::ZERO).with_fwhm(3.2).with_flux(85.0),
        Star::at(DVec2::ZERO).with_fwhm(3.0).with_flux(80.0),
        // Lower flux stars - some outliers
        Star::at(DVec2::ZERO).with_fwhm(3.5).with_flux(50.0), // Keep
        Star::at(DVec2::ZERO).with_fwhm(4.0).with_flux(40.0), // Keep (borderline)
        Star::at(DVec2::ZERO).with_fwhm(8.0).with_flux(30.0), // Remove
        Star::at(DVec2::ZERO).with_fwhm(3.1).with_flux(20.0), // Keep
        Star::at(DVec2::ZERO).with_fwhm(15.0).with_flux(10.0), // Remove
    ];

    let removed = filter_fwhm_outliers(&mut stars, 3.0);

    assert!(removed >= 2, "Should remove at least 2 outliers");
    assert!(
        stars.iter().all(|s| s.fwhm < 8.0),
        "All remaining should have FWHM < 8.0"
    );
}

#[test]
fn filter_fwhm_outliers_preserves_order() {
    // Stars should remain sorted by flux after filtering
    let mut stars: Vec<Star> = vec![
        Star::at(DVec2::ZERO).with_fwhm(3.0).with_flux(100.0),
        Star::at(DVec2::ZERO).with_fwhm(3.1).with_flux(90.0),
        Star::at(DVec2::ZERO).with_fwhm(20.0).with_flux(80.0), // Outlier
        Star::at(DVec2::ZERO).with_fwhm(3.2).with_flux(70.0),
        Star::at(DVec2::ZERO).with_fwhm(3.0).with_flux(60.0),
    ];

    filter_fwhm_outliers(&mut stars, 3.0);

    // Check order is preserved
    for i in 1..stars.len() {
        assert!(
            stars[i - 1].flux >= stars[i].flux,
            "Stars should remain sorted by flux"
        );
    }
}

#[test]
fn filter_fwhm_outliers_stricter_deviation() {
    // Stars: FWHM 3.0, 3.2, 3.4, ..., 4.4 (8 stars) + outliers 6.0, 7.0
    // Strict (1.5) should remove more than loose (5.0)
    let mut stars1: Vec<Star> = (0..8)
        .map(|i| {
            Star::at(DVec2::ZERO)
                .with_fwhm(3.0 + (i as f32 * 0.2))
                .with_flux(100.0 - i as f32)
        })
        .collect();
    stars1.push(Star::at(DVec2::ZERO).with_fwhm(6.0).with_flux(10.0));
    stars1.push(Star::at(DVec2::ZERO).with_fwhm(7.0).with_flux(5.0));

    let mut stars2 = stars1.clone();

    let removed_strict = filter_fwhm_outliers(&mut stars1, 1.5);
    let removed_loose = filter_fwhm_outliers(&mut stars2, 5.0);

    // Reference: first 5 stars (FWHM 3.0, 3.2, 3.4, 3.6, 3.8).
    // median = 3.4, MAD = 0.2, effective_mad = max(0.2, 0.34) = 0.34.
    // Strict: max_fwhm = 3.4 + 1.5 * 0.34 = 3.91 → removes 4.0, 4.2, 4.4, 6.0, 7.0 = 5
    // Loose:  max_fwhm = 3.4 + 5.0 * 0.34 = 5.10 → removes 6.0, 7.0 = 2
    assert!(
        removed_strict > removed_loose,
        "Strict ({}) should remove more than loose ({})",
        removed_strict,
        removed_loose
    );
    assert_eq!(
        removed_loose, 2,
        "Loose should remove 2 outliers (6.0, 7.0)"
    );
    assert_eq!(
        removed_strict, 5,
        "Strict should remove 5 stars (FWHM > 3.91)"
    );
}

#[test]
fn filter_fwhm_outliers_exactly_five_stars() {
    // Minimum number of stars for filtering to work
    let mut stars: Vec<Star> = vec![
        Star::at(DVec2::ZERO).with_fwhm(3.0).with_flux(100.0),
        Star::at(DVec2::ZERO).with_fwhm(3.1).with_flux(90.0),
        Star::at(DVec2::ZERO).with_fwhm(3.0).with_flux(80.0),
        Star::at(DVec2::ZERO).with_fwhm(3.2).with_flux(70.0),
        Star::at(DVec2::ZERO).with_fwhm(20.0).with_flux(60.0), // Outlier
    ];

    let removed = filter_fwhm_outliers(&mut stars, 3.0);

    assert_eq!(removed, 1);
    assert_eq!(stars.len(), 4);
}

#[test]
fn filter_fwhm_outliers_negative_deviation_disabled() {
    let mut stars: Vec<Star> = (0..10)
        .map(|i| {
            Star::at(DVec2::ZERO)
                .with_fwhm(3.0 + i as f32 * 5.0)
                .with_flux(100.0 - i as f32)
        })
        .collect();

    let removed = filter_fwhm_outliers(&mut stars, -1.0);

    assert_eq!(removed, 0);
    assert_eq!(stars.len(), 10);
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
