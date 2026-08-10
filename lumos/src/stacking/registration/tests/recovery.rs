//! Match recovery after an initial transform estimate.

use crate::stacking::registration::ransac::transforms::estimate_transform;
use crate::stacking::registration::recovery::recover_matches;
use crate::stacking::registration::triangle::voting::MatchIndices;
use crate::stacking::registration::*;
use crate::testing::synthetic::transforms::generate_random_positions;

fn identity_matches(count: usize) -> Vec<MatchIndices> {
    (0..count)
        .map(|index| MatchIndices {
            reference: index,
            target: index,
        })
        .collect()
}

/// Apply a similarity transform (rotation + translation) around a center.
fn apply_similarity(pos: DVec2, dx: f64, dy: f64, angle: f64, center: DVec2) -> DVec2 {
    let r = pos - center;
    let cos_a = angle.cos();
    let sin_a = angle.sin();
    DVec2::new(
        cos_a * r.x - sin_a * r.y + center.x + dx,
        sin_a * r.x + cos_a * r.y + center.y + dy,
    )
}

#[test]
fn iterative_recovery_improves_on_biased_seed() {
    // Setup: 50 stars with a rotation transform. Give recover_matches
    // a slightly wrong initial transform (estimated from only 3 seed points).
    // The initial transform is close enough for some matches, but iterative
    // refinement should recover significantly more.
    let ref_stars = generate_random_positions(50, 2000.0, 2000.0, 42);

    let dx = 30.0;
    let dy = -20.0;
    let angle = 1.0_f64.to_radians();
    let center = DVec2::new(1000.0, 1000.0);

    let target_stars: Vec<DVec2> = ref_stars
        .iter()
        .map(|&p| apply_similarity(p, dx, dy, angle, center))
        .collect();

    // Create a biased initial transform from only the first 3 points
    let seed_ref: Vec<DVec2> = ref_stars[..3].to_vec();
    let seed_target: Vec<DVec2> = target_stars[..3].to_vec();
    let initial_transform =
        estimate_transform(&seed_ref, &seed_target, TransformType::Euclidean).unwrap();

    let seed_matches = identity_matches(3);
    let threshold = 3.0; // ~3px

    let RecoveredMatches {
        transform: refined_transform,
        matches: recovered_matches,
    } = recover_matches(
        &ref_stars,
        &target_stars,
        &initial_transform,
        &seed_matches,
        threshold,
        TransformType::Euclidean,
    );

    // Iterative recovery should find many more matches than the 3 seeds
    assert!(
        recovered_matches.len() > 10,
        "Expected significant recovery, got only {} matches from 3 seeds",
        recovered_matches.len()
    );

    // Verify the refined transform is accurate
    let mut max_error = 0.0f64;
    for star_match in &recovered_matches {
        let predicted = refined_transform.apply(ref_stars[star_match.reference]);
        let error = (predicted - target_stars[star_match.target]).length();
        max_error = max_error.max(error);
    }
    assert!(
        max_error < threshold,
        "All recovered matches should be within threshold, max_error={}",
        max_error
    );
}

#[test]
fn iterative_recovery_converges() {
    // With a perfect initial transform, recovery should converge in 1 pass
    // (no improvement possible after first pass finds all matches).
    let ref_stars = generate_random_positions(30, 1000.0, 1000.0, 99);

    let dx = 15.0;
    let dy = -10.0;
    let target_stars: Vec<DVec2> = ref_stars.iter().map(|&p| p + DVec2::new(dx, dy)).collect();

    let transform =
        estimate_transform(&ref_stars, &target_stars, TransformType::Translation).unwrap();

    // Start with only 5 seed matches
    let seed_matches = identity_matches(5);

    let RecoveredMatches {
        matches: recovered, ..
    } = recover_matches(
        &ref_stars,
        &target_stars,
        &transform,
        &seed_matches,
        3.0,
        TransformType::Translation,
    );

    // With a perfect transform, should recover all 30 matches
    assert_eq!(
        recovered.len(),
        30,
        "Perfect transform should recover all stars, got {}",
        recovered.len()
    );
}

#[test]
fn iterative_recovery_never_loses_matches() {
    // Ensure the safety fallback works: we never return fewer matches
    // than we started with.
    let ref_stars = generate_random_positions(20, 1000.0, 1000.0, 77);
    let target_stars: Vec<DVec2> = ref_stars
        .iter()
        .map(|&p| p + DVec2::new(10.0, 5.0))
        .collect();

    let transform =
        estimate_transform(&ref_stars, &target_stars, TransformType::Translation).unwrap();

    let seed_matches = identity_matches(10);

    let RecoveredMatches {
        matches: recovered, ..
    } = recover_matches(
        &ref_stars,
        &target_stars,
        &transform,
        &seed_matches,
        3.0,
        TransformType::Translation,
    );

    assert!(
        recovered.len() >= seed_matches.len(),
        "Should never lose matches: started with {}, got {}",
        seed_matches.len(),
        recovered.len()
    );
}

#[test]
fn iterative_recovery_removes_outliers() {
    // Start with some incorrect seed matches. The re-validation step
    // should remove them during iteration.
    let ref_stars = generate_random_positions(30, 1000.0, 1000.0, 55);
    let target_stars: Vec<DVec2> = ref_stars
        .iter()
        .map(|&p| p + DVec2::new(20.0, -15.0))
        .collect();

    let transform =
        estimate_transform(&ref_stars, &target_stars, TransformType::Translation).unwrap();

    // Good matches plus 2 deliberately wrong matches
    let mut seed_matches = identity_matches(8);
    // Wrong: ref[8] matched to target[15], ref[9] matched to target[20]
    seed_matches.push(MatchIndices {
        reference: 8,
        target: 15,
    });
    seed_matches.push(MatchIndices {
        reference: 9,
        target: 20,
    });

    let RecoveredMatches {
        matches: recovered, ..
    } = recover_matches(
        &ref_stars,
        &target_stars,
        &transform,
        &seed_matches,
        3.0,
        TransformType::Translation,
    );

    // Wrong matches should be removed, correct ones kept
    for star_match in &recovered {
        assert_eq!(
            star_match.reference, star_match.target,
            "All recovered matches should be correct correspondences (r==t for this synthetic data)"
        );
    }

    // Should still recover many correct matches
    assert!(
        recovered.len() >= 20,
        "Should recover many correct matches after removing outliers, got {}",
        recovered.len()
    );
}
