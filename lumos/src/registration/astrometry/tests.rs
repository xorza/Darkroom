//! Integration tests for the astrometry module.

use super::*;

/// Test WCS round-trip at various positions.
#[test]
fn test_wcs_roundtrip_comprehensive() {
    // Create WCS with various parameters
    let test_cases = [
        // (center_ra, center_dec, scale_arcsec, rotation_deg, mirrored)
        (0.0, 0.0, 0.5, 0.0, false),      // Equator, no rotation
        (180.0, 45.0, 1.0, 45.0, false),  // Mid-latitude, 45° rotation
        (270.0, -60.0, 2.0, 90.0, false), // Southern hemisphere
        (90.0, 85.0, 0.3, 180.0, false),  // Near pole
        (180.0, 45.0, 1.0, 0.0, true),    // Mirrored
    ];

    for (ra, dec, scale, rotation, mirrored) in test_cases {
        let wcs = Wcs::from_scale_rotation(
            (512.0, 512.0),
            (ra, dec),
            scale,
            rotation,
            (1024, 1024),
            mirrored,
        );

        // Test multiple pixel positions
        let test_pixels = [
            (0.0, 0.0),
            (512.0, 512.0),
            (1023.0, 1023.0),
            (100.0, 800.0),
            (900.0, 100.0),
        ];

        for (x, y) in test_pixels {
            let (sky_ra, sky_dec) = wcs.pixel_to_sky(x, y);
            let (x2, y2) = wcs.sky_to_pixel(sky_ra, sky_dec);

            assert!(
                (x - x2).abs() < 1e-6,
                "X mismatch at ({}, {}) for WCS(ra={}, dec={}, scale={}, rot={}): {} vs {}",
                x,
                y,
                ra,
                dec,
                scale,
                rotation,
                x,
                x2
            );
            assert!(
                (y - y2).abs() < 1e-6,
                "Y mismatch at ({}, {}) for WCS(ra={}, dec={}, scale={}, rot={}): {} vs {}",
                x,
                y,
                ra,
                dec,
                scale,
                rotation,
                y,
                y2
            );
        }
    }
}

/// Test that quad hashing produces consistent results.
#[test]
fn test_quad_hash_consistency() {
    use glam::DVec2;

    // Create a star pattern
    let positions = vec![
        DVec2::new(100.0, 100.0),
        DVec2::new(200.0, 100.0),
        DVec2::new(150.0, 200.0),
        DVec2::new(250.0, 150.0),
        DVec2::new(50.0, 175.0),
    ];

    let hasher = QuadHasher::new()
        .with_max_stars(5)
        .with_max_quad_radius(200.0);

    let quads1 = hasher.build_quads(&positions);
    let quads2 = hasher.build_quads(&positions);

    // Should produce identical results
    assert_eq!(quads1.len(), quads2.len());

    for (q1, q2) in quads1.iter().zip(quads2.iter()) {
        assert_eq!(q1.star_indices, q2.star_indices);
        for i in 0..4 {
            assert!(
                (q1.code[i] - q2.code[i]).abs() < 1e-10,
                "Code mismatch at {}: {} vs {}",
                i,
                q1.code[i],
                q2.code[i]
            );
        }
    }
}

/// Test quad matching with transformed pattern.
#[test]
fn test_quad_matching_with_transform() {
    use glam::DVec2;

    // Original pattern
    let positions1 = vec![
        DVec2::new(100.0, 100.0),
        DVec2::new(200.0, 100.0),
        DVec2::new(200.0, 200.0),
        DVec2::new(100.0, 200.0),
        DVec2::new(150.0, 150.0), // Center point
    ];

    // Rotated and scaled pattern
    let angle = 30.0_f64.to_radians();
    let scale = 1.2;
    let (sin_a, cos_a) = angle.sin_cos();

    let positions2: Vec<_> = positions1
        .iter()
        .map(|p| {
            let x2 = (p.x * cos_a - p.y * sin_a) * scale + 50.0;
            let y2 = (p.x * sin_a + p.y * cos_a) * scale + 30.0;
            DVec2::new(x2, y2)
        })
        .collect();

    let hasher = QuadHasher::new()
        .with_max_stars(5)
        .with_max_quad_radius(400.0)
        .with_match_tolerance(0.05);

    let quads1 = hasher.build_quads(&positions1);
    let quads2 = hasher.build_quads(&positions2);

    // Should find matching quads
    let matches = hasher.match_quads(&quads1, &quads2);

    assert!(
        !matches.is_empty(),
        "No matches found between original and transformed patterns. \
         quads1={}, quads2={}",
        quads1.len(),
        quads2.len()
    );
}

/// Test catalog angular separation function.
#[test]
fn test_angular_separation_various() {
    use super::catalog::angular_separation;

    // Same point
    assert!(angular_separation(0.0, 0.0, 0.0, 0.0) < 1e-10);

    // 1 degree apart along equator
    let sep = angular_separation(0.0, 0.0, 1.0, 0.0);
    assert!((sep - 1.0).abs() < 1e-10, "Expected 1 deg, got {}", sep);

    // 1 degree apart in declination
    let sep = angular_separation(0.0, 0.0, 0.0, 1.0);
    assert!((sep - 1.0).abs() < 1e-10, "Expected 1 deg, got {}", sep);

    // Poles
    let sep = angular_separation(0.0, 90.0, 180.0, 90.0);
    assert!(sep < 1e-10, "North pole to itself should be 0");

    // North to south pole
    let sep = angular_separation(0.0, 90.0, 0.0, -90.0);
    assert!((sep - 180.0).abs() < 1e-10, "Pole to pole: {}", sep);

    // Across RA=0 boundary
    let sep = angular_separation(359.0, 0.0, 1.0, 0.0);
    assert!((sep - 2.0).abs() < 1e-10, "Across RA=0: {}", sep);
}

/// Test catalog preloaded source filtering.
#[test]
fn test_catalog_preloaded_filtering() {
    let stars = vec![
        CatalogStar::new(180.0, 45.0, 8.0),
        CatalogStar::new(180.1, 45.05, 10.0),
        CatalogStar::new(180.2, 45.1, 12.0),
        CatalogStar::new(181.0, 45.0, 9.0),  // ~0.7 deg away
        CatalogStar::new(180.0, 46.0, 11.0), // 1 deg away
        CatalogStar::new(190.0, 45.0, 7.0),  // Far away
    ];

    let source = CatalogSource::preloaded(stars);

    // Query 0.5 degree radius, all magnitudes
    let result = source.query_region(180.0, 45.0, 0.5, 15.0).unwrap();
    assert_eq!(result.len(), 3, "Expected 3 stars within 0.5 deg");

    // Query with magnitude limit
    let result = source.query_region(180.0, 45.0, 0.5, 9.0).unwrap();
    assert_eq!(result.len(), 1, "Expected 1 star brighter than mag 9");

    // Query with larger radius
    let result = source.query_region(180.0, 45.0, 1.5, 15.0).unwrap();
    assert_eq!(result.len(), 5, "Expected 5 stars within 1.5 deg");

    // Verify sorted by magnitude
    for i in 1..result.len() {
        assert!(
            result[i - 1].mag <= result[i].mag,
            "Stars not sorted by magnitude"
        );
    }
}

/// Test WCS field of view calculation.
#[test]
fn test_wcs_field_of_view() {
    // 1 arcsec/pixel, 3600x3600 pixels = 1x1 degree FOV
    let wcs = Wcs::from_scale_rotation(
        (1800.0, 1800.0),
        (180.0, 45.0),
        1.0,
        0.0,
        (3600, 3600),
        false,
    );

    let (fov_x, fov_y) = wcs.field_of_view();
    assert!(
        (fov_x - 1.0).abs() < 1e-6,
        "FOV X should be 1 deg, got {}",
        fov_x
    );
    assert!(
        (fov_y - 1.0).abs() < 1e-6,
        "FOV Y should be 1 deg, got {}",
        fov_y
    );
}

/// Test WCS builder pattern.
#[test]
fn test_wcs_builder_with_cd() {
    let cd = [[1.0 / 3600.0, 0.0], [0.0, 1.0 / 3600.0]]; // 1 arcsec/pixel

    let wcs = Wcs::builder()
        .crpix(512.0, 512.0)
        .crval(180.0, 45.0)
        .naxis(1024, 1024)
        .cd(cd)
        .build();

    assert_eq!(wcs.crpix, (512.0, 512.0));
    assert_eq!(wcs.crval, (180.0, 45.0));
    assert!((wcs.pixel_scale_arcsec() - 1.0).abs() < 1e-6);
}

/// Test solver error handling.
#[test]
fn test_solver_error_cases() {
    use glam::{DVec2, UVec2};

    // Create solver with preloaded empty catalog
    let config = PlateSolverConfig {
        catalog: CatalogSource::preloaded(vec![]),
        ..Default::default()
    };

    let solver = PlateSolver::new(config);

    // Should fail due to no catalog stars
    let stars = vec![
        DVec2::new(100.0, 100.0),
        DVec2::new(200.0, 200.0),
        DVec2::new(300.0, 300.0),
        DVec2::new(400.0, 400.0),
    ];

    let result = solver.solve(&stars, 180.0, 45.0, 1.0, UVec2::new(1024, 1024));
    assert!(
        matches!(
            result,
            Err(SolveError::CatalogError(CatalogError::NoStarsFound))
        ),
        "Expected CatalogError::NoStarsFound, got {:?}",
        result
    );
}
