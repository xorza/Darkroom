use super::*;

#[test]
fn match_triangles_too_few_points() {
    let two = vec![DVec2::new(0.0, 0.0), DVec2::new(1.0, 0.0)];
    let three = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(1.0, 0.0),
        DVec2::new(0.0, 1.0),
    ];

    // Both sides need >= 3 points
    assert!(match_points(&two, &three, &TriangleConfig::default()).is_empty());
    assert!(match_points(&three, &two, &TriangleConfig::default()).is_empty());
    assert!(match_points(&two, &two, &TriangleConfig::default()).is_empty());
}

#[test]
fn match_triangles_empty_inputs() {
    let empty: Vec<DVec2> = vec![];
    let three = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(1.0, 0.0),
        DVec2::new(0.0, 1.0),
    ];
    assert!(match_points(&empty, &three, &TriangleConfig::default()).is_empty());
    assert!(match_points(&three, &empty, &TriangleConfig::default()).is_empty());
}

#[test]
fn match_identical_star_lists() {
    // 5 points with asymmetric pattern → each matches itself
    let positions = SQUARE_AND_CENTRE.to_vec();

    let matches = match_points(&positions, &positions, &TriangleConfig::default());

    assert_eq!(matches.len(), 5);
    for m in &matches {
        assert_eq!(m.indices.reference, m.indices.target);
    }
}

#[test]
fn match_translated_stars() {
    // Translation preserves triangle ratios → all 5 match
    let ref_positions = SQUARE_AND_CENTRE.to_vec();

    let offset = DVec2::new(100.0, 50.0);
    let target_positions: Vec<DVec2> = ref_positions.iter().map(|p| *p + offset).collect();

    let matches = match_points(
        &ref_positions,
        &target_positions,
        &TriangleConfig::default(),
    );

    assert_eq!(matches.len(), 5);
    for m in &matches {
        assert_eq!(m.indices.reference, m.indices.target);
    }
}

#[test]
fn match_scaled_stars() {
    // Uniform scaling preserves triangle ratios → all 5 match
    let ref_positions = SQUARE_AND_CENTRE.to_vec();

    let target_positions: Vec<DVec2> = ref_positions.iter().map(|p| *p * 2.0).collect();

    let matches = match_points(
        &ref_positions,
        &target_positions,
        &TriangleConfig::default(),
    );

    assert_eq!(matches.len(), 5);
    for m in &matches {
        assert_eq!(m.indices.reference, m.indices.target);
    }
}

#[test]
fn match_rotated_stars() {
    // 90-degree rotation: (x,y) → (-y,x). Preserves ratios. Orientation check off
    // for symmetric pattern to avoid ambiguous correspondence.
    let ref_positions = SQUARE_AND_CENTRE.to_vec();

    let target_positions: Vec<DVec2> = ref_positions
        .iter()
        .map(|p| DVec2::new(-p.y, p.x))
        .collect();

    let config = TriangleConfig {
        check_orientation: false,
        ..Default::default()
    };

    let matches = match_points(&ref_positions, &target_positions, &config);
    assert_eq!(matches.len(), 5);
}

#[test]
fn match_with_missing_stars() {
    // Target has 4 of 5 reference stars → should match exactly 4
    let ref_positions = SQUARE_AND_CENTRE.to_vec();

    let target_positions = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
    ];

    let matches = match_points(
        &ref_positions,
        &target_positions,
        &TriangleConfig::default(),
    );

    assert_eq!(matches.len(), 4);
    for m in &matches {
        assert_eq!(m.indices.reference, m.indices.target);
    }
}

#[test]
fn match_with_extra_stars() {
    // Target has all 4 ref stars plus 2 extras → should match all 4 ref stars
    let ref_positions = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
    ];

    let target_positions = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
        DVec2::new(5.0, 5.0),
        DVec2::new(15.0, 15.0),
    ];

    let matches = match_points(
        &ref_positions,
        &target_positions,
        &TriangleConfig::default(),
    );

    assert_eq!(matches.len(), 4);
    for m in &matches {
        assert_eq!(m.indices.reference, m.indices.target);
    }
}

/// A mirror image flips every triangle's orientation. `IRREGULAR` against its mirror: with the
/// orientation check, no triangle pair agrees and nothing matches; without it each triangle meets
/// its mirror, every point draws its six votes at itself, and all five match.
#[test]
fn the_orientation_check_refuses_a_mirror_image() {
    let mirrored = IRREGULAR.map(|p| DVec2::new(-p.x, p.y));
    for (check_orientation, matched) in [(true, 0), (false, 5)] {
        let config = TriangleConfig {
            check_orientation,
            ..Default::default()
        };
        let matches = match_points(&IRREGULAR, &mirrored, &config);
        assert_eq!(
            matches.len(),
            matched,
            "orientation check {check_orientation}"
        );
        assert!(
            matches
                .iter()
                .all(|m| m.indices.reference == m.indices.target)
        );
    }
}

#[test]
fn match_with_outliers() {
    // 6 real stars + 4 far-away outliers. Matches among real stars should be correct.
    let ref_positions = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(10.0, 0.0),
        DVec2::new(20.0, 0.0),
        DVec2::new(0.0, 10.0),
        DVec2::new(10.0, 10.0),
        DVec2::new(20.0, 10.0),
    ];

    let mut target_positions = ref_positions.clone();
    target_positions.push(DVec2::new(100.0, 100.0));
    target_positions.push(DVec2::new(150.0, 50.0));
    target_positions.push(DVec2::new(75.0, 125.0));
    target_positions.push(DVec2::new(200.0, 200.0));

    let config = TriangleConfig {
        min_votes: 2,
        ..Default::default()
    };

    let matches = match_points(&ref_positions, &target_positions, &config);

    // Should match at least 4 of the 6 real stars
    assert!(
        matches.len() >= 4,
        "With outliers, found only {} matches",
        matches.len()
    );

    // All matches among the 6 real stars should be correct (same index)
    for m in &matches {
        if m.indices.reference < 6 && m.indices.target < 6 {
            assert_eq!(m.indices.reference, m.indices.target);
        }
    }
}

#[test]
fn match_permuted_indices() {
    // Same 5 geometric points, target in reversed order.
    // ref[i] corresponds to target[4-i]
    let points = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(30.0, 0.0),
        DVec2::new(15.0, 40.0),
        DVec2::new(50.0, 20.0),
        DVec2::new(10.0, 25.0),
    ];

    let target_points: Vec<DVec2> = points.iter().rev().copied().collect();

    let config = TriangleConfig {
        min_votes: 1,
        ..Default::default()
    };

    let matches = match_points(&points, &target_points, &config);

    assert!(
        matches.len() >= 4,
        "Should match most points, got {}",
        matches.len()
    );

    for m in &matches {
        let expected_target = 4 - m.indices.reference;
        assert_eq!(
            m.indices.target, expected_target,
            "ref {} should match target {} (same geometric point), got target {}",
            m.indices.reference, expected_target, m.indices.target
        );
    }
}

/// The ratio tolerance decides whether two triangles agree. The 3-4-5 triangle has ratios
/// (0.6, 0.8); a triangle of sides 3, 4.05 and 5 has (0.6, 0.81). Its third vertex sits where
/// circles of radius 3 about one end of the 5-side and 4.05 about the other meet:
/// `x = (25 + 9 − 4.05²)/10`, `y = √(9 − x²)`. A tolerance of 0.005 refuses the pair and nothing
/// matches; 0.02 accepts it and its three vertices match, a vote each.
#[test]
fn the_ratio_tolerance_decides_whether_triangles_agree() {
    let x = (25.0 + 9.0 - 4.05 * 4.05) / 10.0;
    let stretched = [
        DVec2::ZERO,
        DVec2::new(x, (9.0 - x * x).sqrt()),
        DVec2::new(5.0, 0.0),
    ];
    let reference = [DVec2::ZERO, DVec2::new(0.0, 3.0), DVec2::new(4.0, 0.0)];
    for (ratio_tolerance, matched) in [(0.005, 0), (0.02, 3)] {
        let config = TriangleConfig {
            ratio_tolerance,
            min_votes: 1,
            check_orientation: false,
        };
        let matches = match_points(&reference, &stretched, &config);
        assert_eq!(matches.len(), matched, "tolerance {ratio_tolerance}");
    }
}

/// `min_votes` is the evidence a pair needs. Four of `IRREGULAR`'s points against themselves form
/// all four triangles, each point a vertex of three: every point draws exactly three votes, so all
/// four match at `min_votes` 3 and none at 4.
#[test]
fn min_votes_is_the_evidence_a_pair_needs() {
    let four = &IRREGULAR[..4];
    for (min_votes, matched) in [(3, 4), (4, 0)] {
        let config = TriangleConfig {
            min_votes,
            ..Default::default()
        };
        let matches = match_points(four, four, &config);
        assert_eq!(matches.len(), matched, "min_votes {min_votes}");
        assert!(
            matches
                .iter()
                .all(|m| m.indices.reference == m.indices.target)
        );
    }
}

#[test]
fn match_sparse_field_10_stars() {
    // 10 stars in a grid-like pattern with one off-grid point for asymmetry
    let ref_positions = vec![
        DVec2::new(0.0, 0.0),
        DVec2::new(50.0, 0.0),
        DVec2::new(100.0, 0.0),
        DVec2::new(0.0, 50.0),
        DVec2::new(50.0, 50.0),
        DVec2::new(100.0, 50.0),
        DVec2::new(0.0, 100.0),
        DVec2::new(50.0, 100.0),
        DVec2::new(100.0, 100.0),
        DVec2::new(50.0, 25.0), // Breaks grid symmetry
    ];

    let offset = DVec2::new(10.0, 20.0);
    let target_positions: Vec<DVec2> = ref_positions.iter().map(|p| *p + offset).collect();

    let config = TriangleConfig {
        min_votes: 2,
        ..Default::default()
    };

    let matches = match_points(&ref_positions, &target_positions, &config);

    // Translation-invariant matching should find all 10 stars
    assert_eq!(matches.len(), 10);
    for m in &matches {
        assert_eq!(m.indices.reference, m.indices.target);
    }
}

#[test]
fn match_with_subpixel_noise() {
    // 25 irregular positions with +-0.3 pixel noise
    let ref_positions: Vec<DVec2> = (0..25)
        .map(|i| {
            let base_x = f64::from(i % 5) * 80.0 + 100.0;
            let base_y = f64::from(i / 5) * 80.0 + 100.0;
            let jitter_x = (f64::from(i * 13 + 7) * 0.37).sin() * 15.0;
            let jitter_y = (f64::from(i * 17 + 3) * 0.53).cos() * 15.0;
            DVec2::new(base_x + jitter_x, base_y + jitter_y)
        })
        .collect();

    let target_positions: Vec<DVec2> = ref_positions
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let noise_x = ((i * 7 + 3) as f64 * 0.73).sin() * 0.3;
            let noise_y = ((i * 11 + 5) as f64 * 0.91).cos() * 0.3;
            DVec2::new(p.x + noise_x, p.y + noise_y)
        })
        .collect();

    let config = TriangleConfig::default();
    let matches = match_points(&ref_positions, &target_positions, &config);

    // With 80-pixel spacing and 0.3-pixel noise, ratios change by < 0.01 tolerance
    // Should match most of the 25 stars
    assert!(
        matches.len() >= 20,
        "Noisy matching found only {} matches",
        matches.len()
    );

    // All matches should be correct (noise is small relative to spacing)
    for m in &matches {
        assert_eq!(
            m.indices.reference, m.indices.target,
            "Incorrect noisy match: ref {} != target {}",
            m.indices.reference, m.indices.target
        );
    }
}
